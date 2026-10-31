//! `Presenter`: the D3D11 device, the flip-model swap chain and the two
//! passes of `present.hlsl` (4:2:0 or 4:4:4 planes on the GPU, BT.709 in the
//! shader).
//!
//! One picture: three `R8_UNORM` planes are uploaded, `ps_yuv` converts
//! them into a stream-sized RGB texture, and `ps_blit` draws that into the
//! layout rect of the back buffer, which is cleared to black first.
//! `Present(1, 0)` shows it at the next vsync. A device
//! loss is reported, not handled here: the app drops the presenter and
//! builds a new one on the same window (a swap chain is bound to its
//! `HWND`, so the old one must go first).

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::DXGI_STATUS_OCCLUDED;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCOMPILE_OPTIMIZATION_LEVEL3;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D11::D3D11_BIND_RENDER_TARGET;
use windows::Win32::Graphics::Direct3D11::D3D11_BIND_SHADER_RESOURCE;
use windows::Win32::Graphics::Direct3D11::D3D11_COMPARISON_NEVER;
use windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_FLAG;
use windows::Win32::Graphics::Direct3D11::D3D11_FILTER;
use windows::Win32::Graphics::Direct3D11::D3D11_FILTER_MIN_MAG_MIP_LINEAR;
use windows::Win32::Graphics::Direct3D11::D3D11_FILTER_MIN_MAG_MIP_POINT;
use windows::Win32::Graphics::Direct3D11::D3D11_SAMPLER_DESC;
use windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION;
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE_ADDRESS_CLAMP;
use windows::Win32::Graphics::Direct3D11::D3D11_TEXTURE2D_DESC;
use windows::Win32::Graphics::Direct3D11::D3D11_USAGE_DEFAULT;
use windows::Win32::Graphics::Direct3D11::D3D11_VIEWPORT;
use windows::Win32::Graphics::Direct3D11::D3D11CreateDevice;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext;
use windows::Win32::Graphics::Direct3D11::ID3D11PixelShader;
use windows::Win32::Graphics::Direct3D11::ID3D11RenderTargetView;
use windows::Win32::Graphics::Direct3D11::ID3D11SamplerState;
use windows::Win32::Graphics::Direct3D11::ID3D11ShaderResourceView;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Direct3D11::ID3D11VertexShader;
use windows::Win32::Graphics::Dxgi::Common::DXGI_ALPHA_MODE_IGNORE;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8_UNORM;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::Win32::Graphics::Dxgi::CreateDXGIFactory2;
use windows::Win32::Graphics::Dxgi::DXGI_CREATE_FACTORY_FLAGS;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_REMOVED;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_DEVICE_RESET;
use windows::Win32::Graphics::Dxgi::DXGI_ERROR_NOT_FOUND;
use windows::Win32::Graphics::Dxgi::DXGI_GPU_PREFERENCE;
use windows::Win32::Graphics::Dxgi::DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE;
use windows::Win32::Graphics::Dxgi::DXGI_GPU_PREFERENCE_MINIMUM_POWER;
use windows::Win32::Graphics::Dxgi::DXGI_MWA_NO_ALT_ENTER;
use windows::Win32::Graphics::Dxgi::DXGI_PRESENT;
use windows::Win32::Graphics::Dxgi::DXGI_SCALING_STRETCH;
use windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_DESC1;
use windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG;
use windows::Win32::Graphics::Dxgi::DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
use windows::Win32::Graphics::Dxgi::DXGI_SWAP_EFFECT_FLIP_DISCARD;
use windows::Win32::Graphics::Dxgi::DXGI_USAGE_RENDER_TARGET_OUTPUT;
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Graphics::Dxgi::IDXGIAdapter1;
use windows::Win32::Graphics::Dxgi::IDXGIFactory6;
use windows::Win32::Graphics::Dxgi::IDXGISwapChain2;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST;
use windows::Win32::Graphics::Gdi::MonitorFromWindow;
use windows::core::Interface;
use windows::core::PCSTR;
use windows::core::s;

use crate::cli::AdapterChoice;
use crate::layout::Filter;
use crate::layout::Viewport;
use crate::present_log::PresentOutcome;
use crate::present_math::PRESENT_HLSL;
use crate::stats_out::AdapterInfo;
use crate::stats_out::SwapChainInfo;
use crate::yuv::Sampling;
use crate::yuv::YuvFrame;

/// Two buffers, flip-discard: the minimum for flip model, and with a
/// frame latency of 1 the waitable paces us to the display.
const BUFFERS: u32 = 2;
/// The `SyncInterval` of every `Present`.
const SYNC_INTERVAL: u32 = 1;
const MAX_FRAME_LATENCY: u32 = 1;
const BACK_FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
/// The swap chain's flags, kept on every `ResizeBuffers`. No
/// `ALLOW_TEARING`.
const SWAP_FLAGS: DXGI_SWAP_CHAIN_FLAG = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;

/// The stream-sized textures: three planes in, one RGB picture out.
struct Planes {
    size: (u32, u32),
    sampling: Sampling,
    y: ID3D11Texture2D,
    u: ID3D11Texture2D,
    v: ID3D11Texture2D,
    srvs: [Option<ID3D11ShaderResourceView>; 3],
    rgb_rtv: ID3D11RenderTargetView,
    rgb_srv: Option<ID3D11ShaderResourceView>,
}

/// The device, swap chain and pipeline for one window.
pub struct Presenter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    swap_chain: IDXGISwapChain2,
    waitable: HANDLE,
    size: (u32, u32),
    back: Option<ID3D11RenderTargetView>,
    planes: Option<Planes>,
    /// Whether `planes.rgb` holds a converted picture.
    has_picture: bool,
    vs: ID3D11VertexShader,
    ps_yuv: ID3D11PixelShader,
    ps_blit: ID3D11PixelShader,
    point: Option<ID3D11SamplerState>,
    linear: Option<ID3D11SamplerState>,
    adapter: AdapterInfo,
}

impl Presenter {
    /// Device on the chosen adapter, FLIP_DISCARD, 2
    /// buffers, B8G8R8A8, the frame-latency waitable at latency 1,
    /// `DXGI_MWA_NO_ALT_ENTER`; HLSL compiled once with `D3DCompile`.
    pub fn new(hwnd: HWND, size: (u32, u32), choice: AdapterChoice) -> windows::core::Result<Self> {
        // SAFETY: plain factory creation; the flags are valid.
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0))? };
        // SAFETY: `hwnd` is our live window.
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let adapter = pick_adapter(&factory, choice, monitor)?;

        let mut device = None;
        let mut context = None;
        let base: Option<IDXGIAdapter> = adapter.as_ref().map(Interface::cast).transpose()?;
        let driver = match adapter {
            Some(_) => D3D_DRIVER_TYPE_UNKNOWN,
            None => D3D_DRIVER_TYPE_HARDWARE,
        };
        // SAFETY: the out pointers are live locals; an explicit adapter
        // goes with D3D_DRIVER_TYPE_UNKNOWN, none with HARDWARE.
        unsafe {
            D3D11CreateDevice(
                base.as_ref(),
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_FLAG(0),
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )?;
        }
        let device: ID3D11Device = device.expect("D3D11CreateDevice succeeded without a device");
        let context: ID3D11DeviceContext =
            context.expect("D3D11CreateDevice succeeded without a context");
        let info = adapter_info(&device, choice, monitor)?;

        let size = (size.0.max(1), size.1.max(1));
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: BACK_FORMAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: BUFFERS,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: SWAP_FLAGS.0 as u32,
            ..Default::default()
        };
        // SAFETY: `desc` is a live local; `hwnd` has no swap chain (the
        // app drops the previous presenter first).
        let swap_chain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None)? };
        let swap_chain: IDXGISwapChain2 = swap_chain.cast()?;
        // SAFETY: COM calls on the swap chain just created.
        let waitable = unsafe {
            swap_chain.SetMaximumFrameLatency(MAX_FRAME_LATENCY)?;
            factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
            swap_chain.GetFrameLatencyWaitableObject()
        };

        let vs_code = compile(s!("vs_full"), s!("vs_5_0"))?;
        let yuv_code = compile(s!("ps_yuv"), s!("ps_5_0"))?;
        let blit_code = compile(s!("ps_blit"), s!("ps_5_0"))?;
        let mut vs = None;
        let mut ps_yuv = None;
        let mut ps_blit = None;
        // SAFETY: the byte code slices are live blobs from D3DCompile.
        unsafe {
            device.CreateVertexShader(blob_bytes(&vs_code), None, Some(&mut vs))?;
            device.CreatePixelShader(blob_bytes(&yuv_code), None, Some(&mut ps_yuv))?;
            device.CreatePixelShader(blob_bytes(&blit_code), None, Some(&mut ps_blit))?;
        }
        let point = sampler(&device, D3D11_FILTER_MIN_MAG_MIP_POINT)?;
        let linear = sampler(&device, D3D11_FILTER_MIN_MAG_MIP_LINEAR)?;

        let mut presenter = Self {
            device,
            context,
            swap_chain,
            waitable,
            size,
            back: None,
            planes: None,
            has_picture: false,
            vs: vs.expect("CreateVertexShader succeeded without a shader"),
            ps_yuv: ps_yuv.expect("CreatePixelShader succeeded without ps_yuv"),
            ps_blit: ps_blit.expect("CreatePixelShader succeeded without ps_blit"),
            point,
            linear,
            adapter: info,
        };
        presenter.back = Some(presenter.back_view()?);
        Ok(presenter)
    }

    /// The frame-latency waitable: signalled when a new frame may be
    /// presented without queueing behind the display.
    pub fn waitable(&self) -> HANDLE {
        self.waitable
    }

    /// The swap chain's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn adapter(&self) -> &AdapterInfo {
        &self.adapter
    }

    /// The swap chain as created, for the stats file.
    pub fn swap_chain_info(&self) -> SwapChainInfo {
        SwapChainInfo {
            size: self.size,
            swap_effect: "flip_discard".to_string(),
            buffers: BUFFERS,
            allow_tearing: false,
            sync_interval: SYNC_INTERVAL,
            max_frame_latency: MAX_FRAME_LATENCY,
        }
    }

    /// `ResizeBuffers` to the client size, keeping the flags.
    pub fn resize(&mut self, size: (u32, u32)) -> windows::core::Result<()> {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return Ok(());
        }
        self.back = None;
        // SAFETY: the only reference to a back buffer (`back`) is dropped
        // and unbound before ResizeBuffers, as it requires.
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.context.Flush();
            self.swap_chain
                .ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, SWAP_FLAGS)?;
        }
        self.size = size;
        self.back = Some(self.back_view()?);
        Ok(())
    }

    /// Upload `frame`, convert it, draw it into `viewport` and present.
    pub fn present(
        &mut self,
        frame: &YuvFrame,
        viewport: Viewport,
        filter: Filter,
    ) -> windows::core::Result<PresentOutcome> {
        let size = frame.size();
        let sampling = frame.sampling();
        if self
            .planes
            .as_ref()
            .is_none_or(|p| p.size != size || p.sampling != sampling)
        {
            self.planes = None;
            self.has_picture = false;
            self.planes = Some(self.make_planes(size, sampling)?);
        }
        let planes = self.planes.as_ref().expect("planes were just made");
        let (w, h) = size;
        let (cw, _) = sampling.chroma_size(size);
        // SAFETY: each source slice holds exactly one plane of `size` and
        // `sampling` (YuvFrame's invariant: tight strides w and the chroma
        // width, even sides), and the textures were made for both.
        unsafe {
            let ctx = &self.context;
            ctx.UpdateSubresource(&planes.y, 0, None, frame.y().as_ptr().cast(), w, 0);
            ctx.UpdateSubresource(&planes.u, 0, None, frame.u().as_ptr().cast(), cw, 0);
            ctx.UpdateSubresource(&planes.v, 0, None, frame.v().as_ptr().cast(), cw, 0);
            // Pass 1: planes -> stream-sized RGB.
            ctx.PSSetShaderResources(3, Some(&[None]));
            ctx.OMSetRenderTargets(Some(&[Some(planes.rgb_rtv.clone())]), None);
            ctx.RSSetViewports(Some(&[d3d_viewport(Viewport {
                x: 0,
                y: 0,
                width: w,
                height: h,
            })]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps_yuv, None);
            ctx.PSSetShaderResources(0, Some(&planes.srvs));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None, None]));
        }
        self.has_picture = true;
        self.redraw(viewport, filter)
    }

    /// Blit the last picture again (or just black before the first) and
    /// present: `WM_PAINT`, a resize, and the size-move timer.
    pub fn redraw(
        &mut self,
        viewport: Viewport,
        filter: Filter,
    ) -> windows::core::Result<PresentOutcome> {
        let back = self
            .back
            .clone()
            .expect("the presenter always has a back buffer view");
        let sampler = match filter {
            Filter::Point => self.point.clone(),
            Filter::Linear => self.linear.clone(),
        };
        // SAFETY: every bound object belongs to this device; the RGB
        // texture is unbound as a target (OMSetRenderTargets replaces it)
        // before it is bound as a resource.
        let hr = unsafe {
            let ctx = &self.context;
            ctx.OMSetRenderTargets(Some(&[Some(back.clone())]), None);
            ctx.ClearRenderTargetView(&back, &[0.0, 0.0, 0.0, 1.0]);
            if let (true, Some(planes)) = (self.has_picture, self.planes.as_ref())
                && viewport.width > 0
                && viewport.height > 0
            {
                ctx.RSSetViewports(Some(&[d3d_viewport(viewport)]));
                ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                ctx.VSSetShader(&self.vs, None);
                ctx.PSSetShader(&self.ps_blit, None);
                ctx.PSSetShaderResources(3, Some(std::slice::from_ref(&planes.rgb_srv)));
                ctx.PSSetSamplers(0, Some(&[sampler]));
                ctx.Draw(3, 0);
            }
            self.swap_chain.Present(SYNC_INTERVAL, DXGI_PRESENT(0))
        };
        if hr == DXGI_STATUS_OCCLUDED {
            return Ok(PresentOutcome::Occluded);
        }
        if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
            return Ok(PresentOutcome::DeviceLost);
        }
        hr.ok()?;
        Ok(PresentOutcome::Shown)
    }

    fn back_view(&self) -> windows::core::Result<ID3D11RenderTargetView> {
        // SAFETY: buffer 0 of a flip-model swap chain is the current back
        // buffer; the view is made on the device that owns it.
        unsafe {
            let buffer: ID3D11Texture2D = self.swap_chain.GetBuffer(0)?;
            let mut rtv = None;
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut rtv))?;
            Ok(rtv.expect("CreateRenderTargetView succeeded without a view"))
        }
    }

    fn make_planes(&self, (w, h): (u32, u32), sampling: Sampling) -> windows::core::Result<Planes> {
        let chroma = sampling.chroma_size((w, h));
        let y = self.texture((w, h), DXGI_FORMAT_R8_UNORM, Bind::Read)?;
        let u = self.texture(chroma, DXGI_FORMAT_R8_UNORM, Bind::Read)?;
        let v = self.texture(chroma, DXGI_FORMAT_R8_UNORM, Bind::Read)?;
        let rgb = self.texture((w, h), BACK_FORMAT, Bind::ReadWrite)?;
        let mut rtv = None;
        // SAFETY: views on textures of this device, default descs.
        let srvs = unsafe {
            self.device
                .CreateRenderTargetView(&rgb, None, Some(&mut rtv))?;
            [self.srv(&y)?, self.srv(&u)?, self.srv(&v)?]
        };
        Ok(Planes {
            size: (w, h),
            sampling,
            srvs: [
                Some(srvs[0].clone()),
                Some(srvs[1].clone()),
                Some(srvs[2].clone()),
            ],
            y,
            u,
            v,
            rgb_rtv: rtv.expect("CreateRenderTargetView succeeded without a view"),
            rgb_srv: Some(self.srv(&rgb)?),
        })
    }

    fn srv(&self, texture: &ID3D11Texture2D) -> windows::core::Result<ID3D11ShaderResourceView> {
        let mut srv = None;
        // SAFETY: a default view of a texture of this device.
        unsafe {
            self.device
                .CreateShaderResourceView(texture, None, Some(&mut srv))?;
        }
        Ok(srv.expect("CreateShaderResourceView succeeded without a view"))
    }

    fn texture(
        &self,
        (w, h): (u32, u32),
        format: DXGI_FORMAT,
        bind: Bind,
    ) -> windows::core::Result<ID3D11Texture2D> {
        let flags = match bind {
            Bind::Read => D3D11_BIND_SHADER_RESOURCE.0,
            Bind::ReadWrite => D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0,
        };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: w,
            Height: h,
            MipLevels: 1,
            ArraySize: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: flags as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        // SAFETY: `desc` is a live local; no initial data.
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        Ok(texture.expect("CreateTexture2D succeeded without a texture"))
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        // SAFETY: the waitable is ours to close (GetFrameLatencyWaitableObject
        // hands over a handle the caller closes), once.
        let _ = unsafe { CloseHandle(self.waitable) };
    }
}

/// How a texture is bound.
#[derive(Clone, Copy)]
enum Bind {
    /// A shader reads it.
    Read,
    /// A target in one pass and read in the next.
    ReadWrite,
}

fn d3d_viewport(v: Viewport) -> D3D11_VIEWPORT {
    D3D11_VIEWPORT {
        TopLeftX: v.x as f32,
        TopLeftY: v.y as f32,
        Width: v.width as f32,
        Height: v.height as f32,
        MinDepth: 0.0,
        MaxDepth: 1.0,
    }
}

fn sampler(
    device: &ID3D11Device,
    filter: D3D11_FILTER,
) -> windows::core::Result<Option<ID3D11SamplerState>> {
    let desc = D3D11_SAMPLER_DESC {
        Filter: filter,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut state = None;
    // SAFETY: `desc` is a live local.
    unsafe { device.CreateSamplerState(&desc, Some(&mut state))? };
    Ok(state)
}

/// `D3DCompile` one entry point of `present.hlsl`.
fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source pointer and length describe PRESENT_HLSL, a
    // static str; the names are NUL-terminated literals.
    let result = unsafe {
        D3DCompile(
            PRESENT_HLSL.as_ptr().cast(),
            PRESENT_HLSL.len(),
            s!("present.hlsl"),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = result {
        let detail = errors
            .as_ref()
            .map(|b| String::from_utf8_lossy(blob_bytes(b)).into_owned())
            .unwrap_or_default();
        crate::logging::line(&format!("present.hlsl failed to compile: {e} {detail}"));
        return Err(e);
    }
    Ok(code.expect("D3DCompile succeeded without byte code"))
}

fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob's pointer and size describe its own buffer, which
    // lives as long as the blob borrowed here.
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

/// The adapter for `choice`: `None` means the system default (only when
/// no adapter has an output on `monitor`).
fn pick_adapter(
    factory: &IDXGIFactory6,
    choice: AdapterChoice,
    monitor: HMONITOR,
) -> windows::core::Result<Option<IDXGIAdapter1>> {
    let preference: DXGI_GPU_PREFERENCE = match choice {
        AdapterChoice::Monitor => {
            for i in 0.. {
                // SAFETY: enumeration by index; NOT_FOUND ends it.
                let adapter = match unsafe { factory.EnumAdapters1(i) } {
                    Ok(adapter) => adapter,
                    Err(e) if e.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(e) => return Err(e),
                };
                if drives(&adapter, monitor) {
                    return Ok(Some(adapter));
                }
            }
            return Ok(None);
        }
        AdapterChoice::HighPerformance => DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        AdapterChoice::MinimumPower => DXGI_GPU_PREFERENCE_MINIMUM_POWER,
    };
    // SAFETY: index 0 of the preference order always exists when any
    // adapter does.
    let adapter: IDXGIAdapter1 = unsafe { factory.EnumAdapterByGpuPreference(0, preference)? };
    Ok(Some(adapter))
}

/// Whether one of `adapter`'s outputs is `monitor`.
fn drives(adapter: &IDXGIAdapter1, monitor: HMONITOR) -> bool {
    for i in 0.. {
        // SAFETY: enumeration by index; any error ends it.
        let Ok(output) = (unsafe { adapter.EnumOutputs(i) }) else {
            return false;
        };
        // SAFETY: a COM call on a live output.
        if let Ok(desc) = unsafe { output.GetDesc() }
            && desc.Monitor == monitor
        {
            return true;
        }
    }
    false
}

/// `DXGI_ADAPTER_DESC1` of the adapter the device runs on.
fn adapter_info(
    device: &ID3D11Device,
    choice: AdapterChoice,
    monitor: HMONITOR,
) -> windows::core::Result<AdapterInfo> {
    let dxgi: windows::Win32::Graphics::Dxgi::IDXGIDevice = device.cast()?;
    // SAFETY: COM calls on live objects.
    let (adapter, desc) = unsafe {
        let adapter: IDXGIAdapter1 = dxgi.GetAdapter()?.cast()?;
        let desc = adapter.GetDesc1()?;
        (adapter, desc)
    };
    let len = desc
        .Description
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(desc.Description.len());
    Ok(AdapterInfo {
        name: String::from_utf16_lossy(&desc.Description[..len]),
        vendor_id: desc.VendorId,
        device_id: desc.DeviceId,
        choice: choice.name().to_string(),
        drives_monitor: drives(&adapter, monitor),
    })
}
