//! GPU decode: the Windows H.264 decoder (Media Foundation's
//! `CLSID_MSH264DecoderMFT`) on a D3D11 device, so it decodes with D3D11
//! video (DXVA). Each picture is copied back from its NV12 texture into
//! the same tight I420 (or, for Dual420, 4:4:4) `YuvFrame` the CPU path
//! makes, so everything after the decoder is unchanged.
//!
//! H.264 decoding is exact, so the pictures are byte-identical to
//! OpenH264's; `gwin-verify decode --decoder both` checks that.
//!
//! The decoder refuses to run in software: a sample that is not a D3D11
//! texture (`IMFDXGIBuffer`) is an error, and `decode::VideoDecoder` falls
//! back to the CPU.

use std::cell::Cell;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use std::time::Instant;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D11::D3D11_CPU_ACCESS_READ;
use windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_BGRA_SUPPORT;
use windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
use windows::Win32::Graphics::Direct3D11::D3D11_FENCE_FLAG_NONE;
use windows::Win32::Graphics::Direct3D11::D3D11_MAP_READ;
use windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE;
use windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION;
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Direct3D11::D3D11_USAGE_STAGING;
use windows::Win32::Graphics::Direct3D11::D3D11CreateDevice;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D11::ID3D11Device5;
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext;
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext4;
use windows::Win32::Graphics::Direct3D11::ID3D11Fence;
use windows::Win32::Graphics::Direct3D11::ID3D11Multithread;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Media::MediaFoundation::CLSID_MSH264DecoderMFT;
use windows::Win32::Media::MediaFoundation::CODECAPI_AVLowLatencyMode;
use windows::Win32::Media::MediaFoundation::IMFDXGIBuffer;
use windows::Win32::Media::MediaFoundation::IMFDXGIDeviceManager;
use windows::Win32::Media::MediaFoundation::IMFSample;
use windows::Win32::Media::MediaFoundation::IMFTransform;
use windows::Win32::Media::MediaFoundation::MF_E_NOTACCEPTING;
use windows::Win32::Media::MediaFoundation::MF_E_TRANSFORM_NEED_MORE_INPUT;
use windows::Win32::Media::MediaFoundation::MF_E_TRANSFORM_STREAM_CHANGE;
use windows::Win32::Media::MediaFoundation::MF_MT_FRAME_SIZE;
use windows::Win32::Media::MediaFoundation::MF_MT_MAJOR_TYPE;
use windows::Win32::Media::MediaFoundation::MF_MT_SUBTYPE;
use windows::Win32::Media::MediaFoundation::MF_SA_D3D11_AWARE;
use windows::Win32::Media::MediaFoundation::MF_VERSION;
use windows::Win32::Media::MediaFoundation::MFCreateDXGIDeviceManager;
use windows::Win32::Media::MediaFoundation::MFCreateMediaType;
use windows::Win32::Media::MediaFoundation::MFCreateMemoryBuffer;
use windows::Win32::Media::MediaFoundation::MFCreateSample;
use windows::Win32::Media::MediaFoundation::MFMediaType_Video;
use windows::Win32::Media::MediaFoundation::MFSTARTUP_LITE;
use windows::Win32::Media::MediaFoundation::MFSampleExtension_Discontinuity;
use windows::Win32::Media::MediaFoundation::MFStartup;
use windows::Win32::Media::MediaFoundation::MFT_MESSAGE_COMMAND_DRAIN;
use windows::Win32::Media::MediaFoundation::MFT_MESSAGE_NOTIFY_BEGIN_STREAMING;
use windows::Win32::Media::MediaFoundation::MFT_MESSAGE_NOTIFY_START_OF_STREAM;
use windows::Win32::Media::MediaFoundation::MFT_MESSAGE_SET_D3D_MANAGER;
use windows::Win32::Media::MediaFoundation::MFT_OUTPUT_DATA_BUFFER;
use windows::Win32::Media::MediaFoundation::MFT_OUTPUT_STREAM_PROVIDES_SAMPLES;
use windows::Win32::Media::MediaFoundation::MFVideoFormat_H264;
use windows::Win32::Media::MediaFoundation::MFVideoFormat_NV12;
use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;
use windows::Win32::System::Com::COINIT_MULTITHREADED;
use windows::Win32::System::Com::CoCreateInstance;
use windows::Win32::System::Com::CoInitializeEx;
use windows::Win32::System::Threading::CreateEventW;
use windows::Win32::System::Threading::WaitForSingleObject;
use windows::core::Interface;

use gliff_proto::msg::ChromaMode;

use crate::decode::Planes;
use crate::decode::recombine;
use crate::h264;
use crate::yuv::YuvFrame;
use crate::yuv::YuvPool;

/// Why the GPU decoder could not start or stopped; a plain sentence for
/// the log.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{what}: {detail}")]
pub struct GpuError {
    what: &'static str,
    detail: String,
}

impl GpuError {
    fn new(what: &'static str, detail: impl ToString) -> Self {
        Self {
            what,
            detail: detail.to_string(),
        }
    }
}

trait Context<T> {
    fn at(self, what: &'static str) -> Result<T, GpuError>;
}

impl<T> Context<T> for windows::core::Result<T> {
    fn at(self, what: &'static str) -> Result<T, GpuError> {
        self.map_err(|e| GpuError::new(what, format!("{:#010x} {}", e.code().0, e.message())))
    }
}

/// Idle plane buffers: as `decode::POOL_KEEP`, plus the two I420
/// halves of a Dual420 pair.
const POOL_KEEP: usize = 15;

/// One D3D11 device and its Media Foundation device manager, shared by
/// the main and aux decoders.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    manager: IMFDXGIDeviceManager,
    adapter: String,
    /// Waits for the copy-back without spinning; `None` where the device
    /// has no fences, then `Map` waits.
    fence: Option<Fence>,
}

/// A D3D11 fence and an event, to sleep until the GPU has done the work
/// queued so far. `Map` on a busy staging texture spins a CPU core in some
/// drivers for as long as the decode takes.
struct Fence {
    fence: ID3D11Fence,
    context: ID3D11DeviceContext4,
    event: HANDLE,
    value: Cell<u64>,
}

impl Fence {
    fn new(device: &ID3D11Device, context: &ID3D11DeviceContext) -> Result<Self, GpuError> {
        let device: ID3D11Device5 = device.cast().at("fence device")?;
        let context: ID3D11DeviceContext4 = context.cast().at("fence context")?;
        let mut fence: Option<ID3D11Fence> = None;
        // SAFETY: the out pointer is a live local; a null name and no
        // attributes make an unnamed auto-reset event.
        let event = unsafe {
            device
                .CreateFence(0, D3D11_FENCE_FLAG_NONE, &mut fence)
                .at("fence")?;
            CreateEventW(None, false, false, windows::core::PCWSTR::null()).at("fence event")?
        };
        let Some(fence) = fence else {
            // SAFETY: the event was just made and is not used again.
            let _ = unsafe { CloseHandle(event) };
            return Err(GpuError::new("fence", "none"));
        };
        Ok(Self {
            fence,
            context,
            event,
            value: Cell::new(0),
        })
    }

    /// Sleeps until the GPU has done everything queued on the context.
    fn wait(&self) -> Result<(), GpuError> {
        let value = self.value.get() + 1;
        self.value.set(value);
        // SAFETY: plain calls on live interfaces and an event we own.
        unsafe {
            self.context.Signal(&self.fence, value).at("fence signal")?;
            self.fence
                .SetEventOnCompletion(value, self.event)
                .at("fence event")?;
            self.context.Flush();
            match WaitForSingleObject(self.event, 2000) {
                WAIT_OBJECT_0 => Ok(()),
                other => Err(GpuError::new(
                    "copy-back",
                    format!("the GPU did not finish in 2 s ({other:?})"),
                )),
            }
        }
    }
}

impl Drop for Fence {
    fn drop(&mut self) {
        // SAFETY: the event is ours and closed once.
        let _ = unsafe { CloseHandle(self.event) };
    }
}

/// The GPU decoder: one Media Foundation decoder per stream on one
/// device, with the `CpuDecoder` API.
pub struct GpuDecoder {
    // Field order is drop order: the decoders go before the device.
    main: Stream,
    aux: Option<Stream>,
    gpu: Gpu,
    pool: YuvPool,
    held: u32,
}

impl GpuDecoder {
    /// A Single420 decoder on the default adapter, or why there is none.
    pub fn new() -> Result<Self, GpuError> {
        com_and_mf()?;
        let gpu = Gpu::new()?;
        let main = Stream::new(&gpu)?;
        Ok(Self {
            main,
            aux: None,
            gpu,
            pool: YuvPool::new(POOL_KEEP),
            held: 0,
        })
    }

    /// The adapter's name, for the log.
    pub fn adapter(&self) -> &str {
        &self.gpu.adapter
    }

    /// As `CpuDecoder::decode`.
    pub fn decode(&mut self, main: &[u8], aux: &[u8]) -> Result<Option<YuvFrame>, GpuError> {
        let frame = match &mut self.aux {
            None => self.main.decode(&self.gpu, main, &self.pool)?,
            Some(second) => {
                let m = self.main.decode(&self.gpu, main, &self.pool)?;
                let a = second.decode(&self.gpu, aux, &self.pool)?;
                pair(&self.pool, m, a)?
            }
        };
        // Low latency gives one picture per unit; should one call release
        // two, only the newest is shown, but none stays counted as held.
        self.held = (self.held + 1).saturating_sub(self.main.take_released());
        Ok(frame)
    }

    /// As `CpuDecoder::has_pending`.
    pub fn has_pending(&self) -> bool {
        self.held > 0
    }

    /// As `CpuDecoder::flush`: drains what the decoders hold.
    pub fn flush(&mut self) -> Result<Option<YuvFrame>, GpuError> {
        let frame = match &mut self.aux {
            None => self.main.drain(&self.gpu, &self.pool)?,
            Some(second) => {
                let m = self.main.drain(&self.gpu, &self.pool)?;
                let a = second.drain(&self.gpu, &self.pool)?;
                pair(&self.pool, m, a)?
            }
        };
        // Drained: nothing is held any more.
        self.main.take_released();
        self.held = 0;
        Ok(frame)
    }

    /// As `CpuDecoder::reset`: new decoders on the same device.
    pub fn reset(&mut self, chroma: ChromaMode) -> Result<(), GpuError> {
        self.main = Stream::new(&self.gpu)?;
        self.aux = match chroma {
            ChromaMode::Dual420 => Some(Stream::new(&self.gpu)?),
            _ => None,
        };
        self.held = 0;
        Ok(())
    }
}

/// A Dual420 pair from the two halves, by the CPU path's rules.
fn pair(
    pool: &YuvPool,
    main: Option<YuvFrame>,
    aux: Option<YuvFrame>,
) -> Result<Option<YuvFrame>, GpuError> {
    match (main, aux) {
        (Some(m), Some(a)) => recombine(pool, &Planes::of(&m), &Planes::of(&a))
            .map(Some)
            .map_err(|e| GpuError::new("recombine", e)),
        (None, None) => Ok(None),
        _ => Err(GpuError::new(
            "decode",
            "main and aux streams fell out of step",
        )),
    }
}

/// COM for this thread (multithreaded) and Media Foundation for the
/// process, once.
fn com_and_mf() -> Result<(), GpuError> {
    // SAFETY: no reserved pointer. S_FALSE (already initialised) and
    // RPC_E_CHANGED_MODE (an STA thread, where in-process COM still
    // works) are both fine here, so the result is not checked.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    static STARTED: OnceLock<Result<(), GpuError>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            // SAFETY: plain call; it is reference counted and never shut
            // down, so it lives as long as the process.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_LITE) }.at("Media Foundation startup")
        })
        .clone()
}

impl Gpu {
    fn new() -> Result<Self, GpuError> {
        let mut device = None;
        let mut context = None;
        // SAFETY: the out pointers are live locals; no adapter goes with
        // D3D_DRIVER_TYPE_HARDWARE (the default adapter).
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .at("no D3D11 device with video support")?;
        let device: ID3D11Device = device.ok_or_else(|| GpuError::new("D3D11", "no device"))?;
        let context: ID3D11DeviceContext =
            context.ok_or_else(|| GpuError::new("D3D11", "no context"))?;
        // The decoder uses the device from its own threads.
        let mt: ID3D11Multithread = device.cast().at("D3D11 multithread")?;
        // SAFETY: plain call on a live interface.
        let _ = unsafe { mt.SetMultithreadProtected(true) };
        let mut token = 0u32;
        let mut manager = None;
        // SAFETY: the out pointers are live locals.
        unsafe { MFCreateDXGIDeviceManager(&mut token, &mut manager) }.at("DXGI device manager")?;
        let manager: IMFDXGIDeviceManager =
            manager.ok_or_else(|| GpuError::new("DXGI device manager", "none"))?;
        // SAFETY: the device is live; the token is the manager's.
        unsafe { manager.ResetDevice(&device, token) }.at("DXGI device manager reset")?;
        Ok(Self {
            adapter: adapter_name(&device),
            fence: Fence::new(&device, &context).ok(),
            device,
            context,
            manager,
        })
    }
}

fn adapter_name(device: &ID3D11Device) -> String {
    let desc = device
        .cast::<IDXGIDevice>()
        // SAFETY: plain calls on live interfaces.
        .and_then(|d| unsafe { d.GetAdapter() })
        // SAFETY: as above.
        .and_then(|a| unsafe { a.GetDesc() });
    match desc {
        Ok(desc) => {
            let end = desc.Description.iter().position(|&c| c == 0).unwrap_or(128);
            String::from_utf16_lossy(&desc.Description[..end])
        }
        Err(_) => "unknown adapter".to_owned(),
    }
}

/// One Media Foundation H.264 decoder and its copy-back texture.
struct Stream {
    mft: IMFTransform,
    /// Set once the first SPS gave the input and output types.
    streaming: bool,
    /// The display size (after cropping) from the newest SPS.
    display: Option<(u32, u32)>,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    time: i64,
    /// Pictures the decoder handed out since the last `take_released`.
    released: u32,
    /// The next unit is the first after a drain, which Media Foundation
    /// wants marked as a discontinuity.
    after_drain: bool,
}

impl Stream {
    fn new(gpu: &Gpu) -> Result<Self, GpuError> {
        // SAFETY: plain COM creation of an in-process class.
        let mft: IMFTransform =
            unsafe { CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER) }
                .at("the Windows H.264 decoder is not installed")?;
        // SAFETY: plain calls on a live interface.
        unsafe {
            let attrs = mft.GetAttributes().at("decoder attributes")?;
            if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) == 0 {
                return Err(GpuError::new("decoder", "not D3D11 aware"));
            }
            attrs
                .SetUINT32(&CODECAPI_AVLowLatencyMode, 1)
                .at("low-latency mode")?;
            mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, gpu.manager.as_raw() as usize)
                .at("the decoder refused the D3D11 device (no hardware H.264 decoder)")?;
        }
        Ok(Self {
            mft,
            streaming: false,
            display: None,
            staging: None,
            time: 0,
            released: 0,
            after_drain: false,
        })
    }

    /// Sets the input and output types from the first SPS.
    fn start(&mut self, width: u32, height: u32) -> Result<(), GpuError> {
        // SAFETY: plain calls on live interfaces.
        unsafe {
            let input = MFCreateMediaType().at("media type")?;
            input
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .at("input type")?;
            input
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .at("input type")?;
            input
                .SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    (u64::from(width) << 32) | u64::from(height),
                )
                .at("input type")?;
            self.mft.SetInputType(0, &input, 0).at("input type H.264")?;
            self.output_nv12()?;
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .at("begin streaming")?;
            self.mft
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .at("start of stream")?;
            let info = self.mft.GetOutputStreamInfo(0).at("output stream info")?;
            if info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 == 0 {
                return Err(GpuError::new("decoder", "decodes in software only"));
            }
        }
        self.streaming = true;
        Ok(())
    }

    fn output_nv12(&self) -> Result<(), GpuError> {
        for i in 0.. {
            // SAFETY: plain calls on live interfaces.
            let ty = unsafe { self.mft.GetOutputAvailableType(0, i) }.at("no NV12 output")?;
            // SAFETY: as above.
            if unsafe { ty.GetGUID(&MF_MT_SUBTYPE) }.ok() == Some(MFVideoFormat_NV12) {
                // SAFETY: as above; `ty` is one of the MFT's own types.
                return unsafe { self.mft.SetOutputType(0, &ty, 0) }.at("output type NV12");
            }
        }
        unreachable!("the loop returns")
    }

    fn decode(
        &mut self,
        gpu: &Gpu,
        unit: &[u8],
        pool: &YuvPool,
    ) -> Result<Option<YuvFrame>, GpuError> {
        if let Some(Ok(sps)) = h264::find_sps(unit) {
            self.display = Some((sps.width, sps.height));
            if !self.streaming {
                self.start(sps.width, sps.height)?;
            }
        }
        if !self.streaming {
            return Err(GpuError::new("decode", "no SPS before the first picture"));
        }
        let sample = self.sample(unit)?;
        let mut frame = None;
        // SAFETY: plain call on live interfaces.
        match unsafe { self.mft.ProcessInput(0, &sample, 0) } {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_NOTACCEPTING => {
                frame = self.outputs(gpu, pool)?;
                // SAFETY: as above.
                unsafe { self.mft.ProcessInput(0, &sample, 0) }.at("decoder input")?;
            }
            Err(e) => return Err(e).at("decoder input"),
        }
        Ok(self.outputs(gpu, pool)?.or(frame))
    }

    fn drain(&mut self, gpu: &Gpu, pool: &YuvPool) -> Result<Option<YuvFrame>, GpuError> {
        if !self.streaming {
            return Ok(None);
        }
        // SAFETY: plain call on a live interface.
        unsafe { self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0) }.at("drain")?;
        self.after_drain = true;
        self.outputs(gpu, pool)
    }

    fn take_released(&mut self) -> u32 {
        std::mem::take(&mut self.released)
    }

    fn sample(&mut self, unit: &[u8]) -> Result<IMFSample, GpuError> {
        let len = u32::try_from(unit.len()).map_err(|_| GpuError::new("decode", "unit too big"))?;
        // SAFETY: the buffer is locked for exactly `len` bytes while the
        // copy runs, then unlocked.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).at("input buffer")?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None).at("input buffer lock")?;
            std::ptr::copy_nonoverlapping(unit.as_ptr(), ptr, unit.len());
            buffer.Unlock().at("input buffer unlock")?;
            buffer.SetCurrentLength(len).at("input buffer length")?;
            let sample = MFCreateSample().at("input sample")?;
            sample.AddBuffer(&buffer).at("input sample")?;
            // 60 fps in 100 ns units; the decoder needs times, not real ones.
            sample.SetSampleTime(self.time).at("sample time")?;
            sample.SetSampleDuration(166_667).at("sample time")?;
            self.time += 166_667;
            if std::mem::take(&mut self.after_drain) {
                sample
                    .SetUINT32(&MFSampleExtension_Discontinuity, 1)
                    .at("sample discontinuity")?;
            }
            Ok(sample)
        }
    }

    /// Every picture the decoder has ready; the newest is kept, as the
    /// frame slot would keep it.
    fn outputs(&mut self, gpu: &Gpu, pool: &YuvPool) -> Result<Option<YuvFrame>, GpuError> {
        let mut newest = None;
        loop {
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(None),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            // SAFETY: one output buffer; the decoder provides the sample.
            let result = unsafe { self.mft.ProcessOutput(0, &mut out, &mut status) };
            // SAFETY: each field is taken once, so the references the
            // decoder handed over are released exactly once.
            let (sample, _events) = unsafe {
                (
                    ManuallyDrop::take(&mut out[0].pSample),
                    ManuallyDrop::take(&mut out[0].pEvents),
                )
            };
            match result {
                Ok(()) => {
                    if let Some(sample) = sample {
                        newest = Some(self.read_back(gpu, &sample, pool)?);
                        self.released += 1;
                    }
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(newest),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.output_nv12()?,
                Err(e) => return Err(e).at("decoder output"),
            }
        }
    }

    /// Copies the sample's NV12 texture into the staging texture, maps it
    /// and splits it into tight I420 at the even display size.
    fn read_back(
        &mut self,
        gpu: &Gpu,
        sample: &IMFSample,
        pool: &YuvPool,
    ) -> Result<YuvFrame, GpuError> {
        // SAFETY: plain calls on live interfaces; the texture pointer is
        // checked and adopted once; the mapped rows are read inside the
        // mapped size (pitch times the texture height, then half that for
        // the chroma plane), and unmapped before return.
        unsafe {
            let buffer = sample.GetBufferByIndex(0).at("output buffer")?;
            let dxgi: IMFDXGIBuffer = buffer.cast().map_err(|_| {
                GpuError::new("decoder", "output is not a GPU texture (software decode)")
            })?;
            let mut raw = std::ptr::null_mut();
            dxgi.GetResource(&ID3D11Texture2D::IID, &mut raw)
                .at("output texture")?;
            let texture = ID3D11Texture2D::from_raw(raw);
            let index = dxgi.GetSubresourceIndex().at("output subresource")?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if desc.Format != DXGI_FORMAT_NV12 {
                return Err(GpuError::new(
                    "decoder",
                    format!("output is {:?}, not NV12", desc.Format),
                ));
            }
            let (dw, dh) = self.display.unwrap_or((desc.Width, desc.Height));
            let w = dw.min(desc.Width) as usize & !1;
            let h = dh.min(desc.Height) as usize & !1;
            let staging = self.staging(gpu, desc.Width, desc.Height)?;
            gpu.context
                .CopySubresourceRegion(&staging, 0, 0, 0, 0, &texture, index, None);
            // Sleep until the copy is done, so `Map` does not spin.
            if let Some(fence) = &gpu.fence {
                fence.wait()?;
            }
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .at("map the copy-back texture")?;
            let pitch = mapped.RowPitch as usize;
            let rows = desc.Height as usize;
            let base = mapped.pData as *const u8;
            let luma = std::slice::from_raw_parts(base, pitch * rows);
            let chroma = std::slice::from_raw_parts(base.add(pitch * rows), pitch * rows / 2);
            let frame = nv12_to_i420(pool, luma, chroma, pitch, w, h);
            gpu.context.Unmap(&staging, 0);
            frame
        }
    }

    fn staging(&mut self, gpu: &Gpu, width: u32, height: u32) -> Result<ID3D11Texture2D, GpuError> {
        if let Some((t, w, h)) = &self.staging
            && (*w, *h) == (width, height)
        {
            return Ok(t.clone());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_NV12,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: the descriptor and out pointer are live locals.
        unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut texture)) }
            .at("copy-back texture")?;
        let texture = texture.ok_or_else(|| GpuError::new("copy-back texture", "none"))?;
        self.staging = Some((texture.clone(), width, height));
        Ok(texture)
    }
}

/// Tight I420 at `w` x `h` from an NV12 picture whose rows are `pitch`
/// bytes apart.
fn nv12_to_i420(
    pool: &YuvPool,
    luma: &[u8],
    chroma: &[u8],
    pitch: usize,
    w: usize,
    h: usize,
) -> Result<YuvFrame, GpuError> {
    let (cw, ch) = (w / 2, h / 2);
    let mut y = pool.get(w * h);
    let mut u = pool.get(cw * ch);
    let mut v = pool.get(cw * ch);
    for (row, out) in y
        .as_mut_slice()
        .chunks_exact_mut(w.max(1))
        .take(h)
        .enumerate()
    {
        out.copy_from_slice(&luma[row * pitch..row * pitch + w]);
    }
    let rows = u
        .as_mut_slice()
        .chunks_exact_mut(cw.max(1))
        .zip(v.as_mut_slice().chunks_exact_mut(cw.max(1)))
        .take(ch);
    for (row, (u_row, v_row)) in rows.enumerate() {
        let src = &chroma[row * pitch..row * pitch + 2 * cw];
        for ((pair, u), v) in src.as_chunks::<2>().0.iter().zip(u_row).zip(v_row) {
            *u = pair[0];
            *v = pair[1];
        }
    }
    let size = (w as u32, h as u32);
    YuvFrame::new(size, y, u, v, Instant::now()).map_err(|e| GpuError::new("decoded picture", e))
}
