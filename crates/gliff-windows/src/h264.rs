//! Just enough H.264 to report what a stream signals: Annex B NAL
//! splitting and the SPS header fields (ITU-T H.264 §7.3.2.1.1), and to
//! tell a decoder that a stream never reorders (§E.1.1).

/// The highest `level_idc` the OpenH264 decoder accepts (5.2). Above it,
/// every unit fails with dsNoParamSets (libx264's level 6.0 clips).
pub const OPENH264_MAX_LEVEL_IDC: u8 = 52;

/// SPS fields `gwin-verify stream` reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpsInfo {
    pub profile_idc: u8,
    pub constraint_flags: u8,
    pub level_idc: u8,
    /// Display size after cropping.
    pub width: u32,
    pub height: u32,
}

impl SpsInfo {
    /// "High", "Baseline", ... or "profile N".
    pub fn profile_name(&self) -> String {
        let constrained = self.constraint_flags & 0x40 != 0;
        match (self.profile_idc, constrained) {
            (66, true) => "Constrained Baseline".to_owned(),
            (66, false) => "Baseline".to_owned(),
            (77, _) => "Main".to_owned(),
            (88, _) => "Extended".to_owned(),
            (100, _) => "High".to_owned(),
            (110, _) => "High 10".to_owned(),
            (122, _) => "High 4:2:2".to_owned(),
            (244, _) => "High 4:4:4".to_owned(),
            (other, _) => format!("profile {other}"),
        }
    }

    /// "5.2" for 52; level 1b (11 with constraint_set3 in Baseline/Main)
    /// reads "1b".
    pub fn level_name(&self) -> String {
        let one_b = self.level_idc == 11
            && matches!(self.profile_idc, 66 | 77 | 88)
            && self.constraint_flags & 0x10 != 0;
        if one_b {
            "1b".to_owned()
        } else {
            format!("{}.{}", self.level_idc / 10, self.level_idc % 10)
        }
    }

    /// Whether the OpenH264 decoder takes this stream.
    pub fn decoder_accepts(&self) -> LevelVerdict {
        if self.level_idc <= OPENH264_MAX_LEVEL_IDC {
            LevelVerdict::Accepted
        } else {
            LevelVerdict::Rejected
        }
    }
}

/// See [`SpsInfo::decoder_accepts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelVerdict {
    Accepted,
    Rejected,
}

/// Why an SPS could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpsError {
    #[error("SPS ends early")]
    Truncated,
    #[error("SPS field out of range: {0}")]
    Invalid(&'static str),
}

/// The NAL units of an Annex B access unit, without start codes.
pub fn nal_units(au: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (n, &(_, payload)) in starts.iter().enumerate() {
        let end = match starts.get(n + 1) {
            Some(&(next, _)) => next,
            None => au.len(),
        };
        let mut end = end;
        while end > payload && au[end - 1] == 0 {
            end -= 1;
        }
        units.push(&au[payload..end]);
    }
    units
}

/// The first SPS in an access unit, parsed; `None` when it has no SPS.
pub fn find_sps(au: &[u8]) -> Option<Result<SpsInfo, SpsError>> {
    nal_units(au)
        .into_iter()
        .find(|nal| nal.first().is_some_and(|h| h & 0x1f == 7))
        .map(parse_sps)
}

/// Parses an SPS NAL unit (header byte included).
pub fn parse_sps(nal: &[u8]) -> Result<SpsInfo, SpsError> {
    let rbsp = unescape(nal.get(1..).ok_or(SpsError::Truncated)?);
    parse_seq_parameter_set(&rbsp)?.sps_info()
}

/// The access unit with VUI bitstream restrictions (no reordering, a
/// decoded picture buffer of `max_num_ref_frames`) added to every SPS that
/// has no VUI; `None` when no SPS needed it.
///
/// Without them, a decoder must assume a POC type 0 stream may reorder,
/// and the Windows decoder then holds pictures until its whole picture
/// buffer is full. An SPS that has a VUI is left alone.
pub fn without_reorder_delay(au: &[u8]) -> Option<Vec<u8>> {
    let nals = nal_units(au);
    let rewritten: Vec<Option<Vec<u8>>> = nals.iter().map(|nal| restrict_sps(nal)).collect();
    if rewritten.iter().all(Option::is_none) {
        return None;
    }
    let mut out = Vec::with_capacity(au.len() + 8 * nals.len());
    for (nal, new) in nals.iter().zip(&rewritten) {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(new.as_deref().unwrap_or(nal));
    }
    Some(out)
}

/// One SPS NAL with a VUI that holds only the bitstream restrictions;
/// `None` for any other NAL, an SPS that already has a VUI, or one that
/// does not parse.
fn restrict_sps(nal: &[u8]) -> Option<Vec<u8>> {
    if nal.first()? & 0x1f != 7 {
        return None;
    }
    let rbsp = unescape(&nal[1..]);
    let mut sps = parse_seq_parameter_set(&rbsp).ok()?;
    if sps.vui_parameters_present_flag {
        return None;
    }
    sps.vui_parameters_present_flag = true;
    sps.vui = Some(VuiParameters::no_reorder(sps.max_num_ref_frames));
    let mut w = BitWriter::default();
    sps.write(&mut w);
    let mut out = vec![nal[0]];
    out.extend(escape(&w.finish()));
    Some(out)
}

/// `seq_parameter_set_data` (ITU-T H.264 §7.3.2.1.1).
///
/// A value parsed from a bitstream has `vui` of `None`. [`restrict_sps`]
/// sets `vui_parameters_present_flag` and fills `vui`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SeqParameterSet {
    profile_idc: u8,
    constraint_set0_flag: bool,
    constraint_set1_flag: bool,
    constraint_set2_flag: bool,
    constraint_set3_flag: bool,
    constraint_set4_flag: bool,
    constraint_set5_flag: bool,
    reserved_zero_2bits: u8,
    level_idc: u8,
    seq_parameter_set_id: u32,
    high: Option<HighProfile>,
    log2_max_frame_num_minus4: u32,
    pic_order_cnt: PicOrderCnt,
    max_num_ref_frames: u32,
    gaps_in_frame_num_value_allowed_flag: bool,
    pic_width_in_mbs_minus1: u32,
    pic_height_in_map_units_minus1: u32,
    frame_mbs_only_flag: bool,
    mb_adaptive_frame_field_flag: bool,
    direct_8x8_inference_flag: bool,
    frame_cropping_flag: bool,
    frame_crop_left_offset: u32,
    frame_crop_right_offset: u32,
    frame_crop_top_offset: u32,
    frame_crop_bottom_offset: u32,
    vui_parameters_present_flag: bool,
    vui: Option<VuiParameters>,
}

/// Fields carried only by the profiles that encode `chroma_format_idc`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HighProfile {
    chroma_format_idc: u32,
    separate_colour_plane_flag: bool,
    bit_depth_luma_minus8: u32,
    bit_depth_chroma_minus8: u32,
    qpprime_y_zero_transform_bypass_flag: bool,
    seq_scaling_matrix_present_flag: bool,
    /// One entry per list when `seq_scaling_matrix_present_flag` is set.
    /// `None` is a clear flag. `Some` holds the `delta_scale` values coded
    /// for that list.
    seq_scaling_list_present_flag: Vec<Option<Vec<i32>>>,
}

/// `pic_order_cnt_type` and the elements that follow it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PicOrderCnt {
    Type0 {
        log2_max_pic_order_cnt_lsb_minus4: u32,
    },
    Type1 {
        delta_pic_order_always_zero_flag: bool,
        offset_for_non_ref_pic: i32,
        offset_for_top_to_bottom_field: i32,
        offset_for_ref_frame: Vec<i32>,
    },
    Type2,
}

/// `vui_parameters` (§E.1.1). `write` emits every present-flag and, when
/// `bitstream_restriction_flag` is set, the bitstream-restriction elements.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VuiParameters {
    aspect_ratio_info_present_flag: bool,
    overscan_info_present_flag: bool,
    video_signal_type_present_flag: bool,
    chroma_loc_info_present_flag: bool,
    timing_info_present_flag: bool,
    nal_hrd_parameters_present_flag: bool,
    vcl_hrd_parameters_present_flag: bool,
    pic_struct_present_flag: bool,
    bitstream_restriction_flag: bool,
    motion_vectors_over_pic_boundaries_flag: bool,
    max_bytes_per_pic_denom: u32,
    max_bits_per_mb_denom: u32,
    log2_max_mv_length_horizontal: u32,
    log2_max_mv_length_vertical: u32,
    max_num_reorder_frames: u32,
    max_dec_frame_buffering: u32,
}

impl VuiParameters {
    /// No reordering, and a decoded picture buffer of `max_num_ref_frames`
    /// (at least one).
    fn no_reorder(max_num_ref_frames: u32) -> Self {
        Self {
            aspect_ratio_info_present_flag: false,
            overscan_info_present_flag: false,
            video_signal_type_present_flag: false,
            chroma_loc_info_present_flag: false,
            timing_info_present_flag: false,
            nal_hrd_parameters_present_flag: false,
            vcl_hrd_parameters_present_flag: false,
            pic_struct_present_flag: false,
            bitstream_restriction_flag: true,
            motion_vectors_over_pic_boundaries_flag: true,
            max_bytes_per_pic_denom: 2,
            max_bits_per_mb_denom: 1,
            log2_max_mv_length_horizontal: 16,
            log2_max_mv_length_vertical: 16,
            max_num_reorder_frames: 0,
            max_dec_frame_buffering: max_num_ref_frames.max(1),
        }
    }

    fn write(&self, w: &mut BitWriter) {
        w.flag(self.aspect_ratio_info_present_flag);
        w.flag(self.overscan_info_present_flag);
        w.flag(self.video_signal_type_present_flag);
        w.flag(self.chroma_loc_info_present_flag);
        w.flag(self.timing_info_present_flag);
        w.flag(self.nal_hrd_parameters_present_flag);
        w.flag(self.vcl_hrd_parameters_present_flag);
        w.flag(self.pic_struct_present_flag);
        w.flag(self.bitstream_restriction_flag);
        if self.bitstream_restriction_flag {
            w.flag(self.motion_vectors_over_pic_boundaries_flag);
            w.ue(self.max_bytes_per_pic_denom);
            w.ue(self.max_bits_per_mb_denom);
            w.ue(self.log2_max_mv_length_horizontal);
            w.ue(self.log2_max_mv_length_vertical);
            w.ue(self.max_num_reorder_frames);
            w.ue(self.max_dec_frame_buffering);
        }
    }
}

impl SeqParameterSet {
    fn sps_info(&self) -> Result<SpsInfo, SpsError> {
        let chroma_format_idc = match &self.high {
            Some(high) => high.chroma_format_idc,
            None => 1,
        };
        let (sub_w, sub_h) = match chroma_format_idc {
            1 => (2, 2),
            2 => (2, 1),
            _ => (1, 1),
        };
        let frame_mbs_only = u32::from(self.frame_mbs_only_flag);
        let field = 2 - frame_mbs_only;
        let (crop_x, crop_y) = match chroma_format_idc {
            0 => (1, field),
            _ => (sub_w, sub_h * field),
        };
        let coded_w = (self.pic_width_in_mbs_minus1 + 1) * 16;
        let coded_h = (self.pic_height_in_map_units_minus1 + 1) * 16 * field;
        let (left, right, top, bottom) = if self.frame_cropping_flag {
            (
                self.frame_crop_left_offset,
                self.frame_crop_right_offset,
                self.frame_crop_top_offset,
                self.frame_crop_bottom_offset,
            )
        } else {
            (0, 0, 0, 0)
        };
        let crop_w = (left + right) * crop_x;
        let crop_h = (top + bottom) * crop_y;
        if crop_w >= coded_w || crop_h >= coded_h {
            return Err(SpsError::Invalid("frame cropping"));
        }
        Ok(SpsInfo {
            profile_idc: self.profile_idc,
            constraint_flags: self.constraint_flags(),
            level_idc: self.level_idc,
            width: coded_w - crop_w,
            height: coded_h - crop_h,
        })
    }

    fn constraint_flags(&self) -> u8 {
        u8::from(self.constraint_set0_flag) << 7
            | u8::from(self.constraint_set1_flag) << 6
            | u8::from(self.constraint_set2_flag) << 5
            | u8::from(self.constraint_set3_flag) << 4
            | u8::from(self.constraint_set4_flag) << 3
            | u8::from(self.constraint_set5_flag) << 2
            | (self.reserved_zero_2bits & 0x03)
    }

    fn write(&self, w: &mut BitWriter) {
        w.u(8, u32::from(self.profile_idc));
        w.flag(self.constraint_set0_flag);
        w.flag(self.constraint_set1_flag);
        w.flag(self.constraint_set2_flag);
        w.flag(self.constraint_set3_flag);
        w.flag(self.constraint_set4_flag);
        w.flag(self.constraint_set5_flag);
        w.u(2, u32::from(self.reserved_zero_2bits));
        w.u(8, u32::from(self.level_idc));
        w.ue(self.seq_parameter_set_id);
        if let Some(high) = &self.high {
            high.write(w);
        }
        w.ue(self.log2_max_frame_num_minus4);
        self.pic_order_cnt.write(w);
        w.ue(self.max_num_ref_frames);
        w.flag(self.gaps_in_frame_num_value_allowed_flag);
        w.ue(self.pic_width_in_mbs_minus1);
        w.ue(self.pic_height_in_map_units_minus1);
        w.flag(self.frame_mbs_only_flag);
        if !self.frame_mbs_only_flag {
            w.flag(self.mb_adaptive_frame_field_flag);
        }
        w.flag(self.direct_8x8_inference_flag);
        w.flag(self.frame_cropping_flag);
        if self.frame_cropping_flag {
            w.ue(self.frame_crop_left_offset);
            w.ue(self.frame_crop_right_offset);
            w.ue(self.frame_crop_top_offset);
            w.ue(self.frame_crop_bottom_offset);
        }
        w.flag(self.vui_parameters_present_flag);
        if let Some(vui) = &self.vui {
            vui.write(w);
        }
        w.rbsp_trailing_bits();
    }
}

impl HighProfile {
    fn write(&self, w: &mut BitWriter) {
        w.ue(self.chroma_format_idc);
        if self.chroma_format_idc == 3 {
            w.flag(self.separate_colour_plane_flag);
        }
        w.ue(self.bit_depth_luma_minus8);
        w.ue(self.bit_depth_chroma_minus8);
        w.flag(self.qpprime_y_zero_transform_bypass_flag);
        w.flag(self.seq_scaling_matrix_present_flag);
        if self.seq_scaling_matrix_present_flag {
            for (i, list) in self.seq_scaling_list_present_flag.iter().enumerate() {
                match list {
                    Some(delta_scale) => {
                        let seq_scaling_list_present_flag = true;
                        w.flag(seq_scaling_list_present_flag);
                        let size = if i < 6 { 16 } else { 64 };
                        write_scaling_list(w, size, delta_scale);
                    }
                    None => {
                        let seq_scaling_list_present_flag = false;
                        w.flag(seq_scaling_list_present_flag);
                    }
                }
            }
        }
    }
}

impl PicOrderCnt {
    fn write(&self, w: &mut BitWriter) {
        match self {
            Self::Type0 {
                log2_max_pic_order_cnt_lsb_minus4,
            } => {
                let pic_order_cnt_type = 0;
                w.ue(pic_order_cnt_type);
                w.ue(*log2_max_pic_order_cnt_lsb_minus4);
            }
            Self::Type1 {
                delta_pic_order_always_zero_flag,
                offset_for_non_ref_pic,
                offset_for_top_to_bottom_field,
                offset_for_ref_frame,
            } => {
                let pic_order_cnt_type = 1;
                w.ue(pic_order_cnt_type);
                w.flag(*delta_pic_order_always_zero_flag);
                w.se(*offset_for_non_ref_pic);
                w.se(*offset_for_top_to_bottom_field);
                let num_ref_frames_in_pic_order_cnt_cycle =
                    u32::try_from(offset_for_ref_frame.len())
                        .expect("num_ref_frames_in_pic_order_cnt_cycle");
                w.ue(num_ref_frames_in_pic_order_cnt_cycle);
                for offset_for_ref_frame in offset_for_ref_frame {
                    w.se(*offset_for_ref_frame);
                }
            }
            Self::Type2 => {
                let pic_order_cnt_type = 2;
                w.ue(pic_order_cnt_type);
            }
        }
    }
}

fn profile_carries_chroma_format(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn scaling_list_count(chroma_format_idc: u32) -> usize {
    if chroma_format_idc == 3 { 12 } else { 8 }
}

fn parse_seq_parameter_set(rbsp: &[u8]) -> Result<SeqParameterSet, SpsError> {
    let mut r = BitReader::new(rbsp);
    parse_seq_parameter_set_data(&mut r)
}

fn parse_seq_parameter_set_data(r: &mut BitReader<'_>) -> Result<SeqParameterSet, SpsError> {
    let profile_idc = r.u8()?;
    let constraint_flags = r.u8()?;
    let constraint_set0_flag = constraint_flags & 0x80 != 0;
    let constraint_set1_flag = constraint_flags & 0x40 != 0;
    let constraint_set2_flag = constraint_flags & 0x20 != 0;
    let constraint_set3_flag = constraint_flags & 0x10 != 0;
    let constraint_set4_flag = constraint_flags & 0x08 != 0;
    let constraint_set5_flag = constraint_flags & 0x04 != 0;
    let reserved_zero_2bits = constraint_flags & 0x03;
    let level_idc = r.u8()?;
    let seq_parameter_set_id = r.ue()?;
    let high = if profile_carries_chroma_format(profile_idc) {
        Some(parse_high_profile(r)?)
    } else {
        None
    };
    let log2_max_frame_num_minus4 = r.ue()?;
    let pic_order_cnt = parse_pic_order_cnt(r)?;
    let max_num_ref_frames = r.ue()?;
    let gaps_in_frame_num_value_allowed_flag = r.flag()?;
    let pic_width_in_mbs_minus1 = r.ue()?;
    let pic_height_in_map_units_minus1 = r.ue()?;
    let frame_mbs_only_flag = r.flag()?;
    let mb_adaptive_frame_field_flag = if frame_mbs_only_flag {
        false
    } else {
        r.flag()?
    };
    let direct_8x8_inference_flag = r.flag()?;
    let frame_cropping_flag = r.flag()?;
    let (
        frame_crop_left_offset,
        frame_crop_right_offset,
        frame_crop_top_offset,
        frame_crop_bottom_offset,
    ) = if frame_cropping_flag {
        (r.ue()?, r.ue()?, r.ue()?, r.ue()?)
    } else {
        (0, 0, 0, 0)
    };
    let mut sps = SeqParameterSet {
        profile_idc,
        constraint_set0_flag,
        constraint_set1_flag,
        constraint_set2_flag,
        constraint_set3_flag,
        constraint_set4_flag,
        constraint_set5_flag,
        reserved_zero_2bits,
        level_idc,
        seq_parameter_set_id,
        high,
        log2_max_frame_num_minus4,
        pic_order_cnt,
        max_num_ref_frames,
        gaps_in_frame_num_value_allowed_flag,
        pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1,
        frame_mbs_only_flag,
        mb_adaptive_frame_field_flag,
        direct_8x8_inference_flag,
        frame_cropping_flag,
        frame_crop_left_offset,
        frame_crop_right_offset,
        frame_crop_top_offset,
        frame_crop_bottom_offset,
        vui_parameters_present_flag: false,
        vui: None,
    };
    sps.sps_info()?;
    sps.vui_parameters_present_flag = r.flag()?;
    Ok(sps)
}

fn parse_high_profile(r: &mut BitReader<'_>) -> Result<HighProfile, SpsError> {
    let chroma_format_idc = r.ue()?;
    if chroma_format_idc > 3 {
        return Err(SpsError::Invalid("chroma_format_idc"));
    }
    let separate_colour_plane_flag = if chroma_format_idc == 3 {
        r.flag()?
    } else {
        false
    };
    let bit_depth_luma_minus8 = r.ue()?;
    let bit_depth_chroma_minus8 = r.ue()?;
    let qpprime_y_zero_transform_bypass_flag = r.flag()?;
    let seq_scaling_matrix_present_flag = r.flag()?;
    let mut seq_scaling_list_present_flag = Vec::new();
    if seq_scaling_matrix_present_flag {
        let count = scaling_list_count(chroma_format_idc);
        seq_scaling_list_present_flag.reserve(count);
        for i in 0..count {
            if r.flag()? {
                let size = if i < 6 { 16 } else { 64 };
                seq_scaling_list_present_flag.push(Some(read_scaling_list(r, size)?));
            } else {
                seq_scaling_list_present_flag.push(None);
            }
        }
    }
    Ok(HighProfile {
        chroma_format_idc,
        separate_colour_plane_flag,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        qpprime_y_zero_transform_bypass_flag,
        seq_scaling_matrix_present_flag,
        seq_scaling_list_present_flag,
    })
}

fn parse_pic_order_cnt(r: &mut BitReader<'_>) -> Result<PicOrderCnt, SpsError> {
    match r.ue()? {
        0 => Ok(PicOrderCnt::Type0 {
            log2_max_pic_order_cnt_lsb_minus4: r.ue()?,
        }),
        1 => {
            let delta_pic_order_always_zero_flag = r.flag()?;
            let offset_for_non_ref_pic = r.se()?;
            let offset_for_top_to_bottom_field = r.se()?;
            let num_ref_frames_in_pic_order_cnt_cycle = r.ue()?;
            if num_ref_frames_in_pic_order_cnt_cycle > 255 {
                return Err(SpsError::Invalid("num_ref_frames_in_pic_order_cnt_cycle"));
            }
            let count = usize::try_from(num_ref_frames_in_pic_order_cnt_cycle)
                .expect("num_ref_frames_in_pic_order_cnt_cycle");
            let mut offset_for_ref_frame = Vec::with_capacity(count);
            for _ in 0..count {
                offset_for_ref_frame.push(r.se()?);
            }
            Ok(PicOrderCnt::Type1 {
                delta_pic_order_always_zero_flag,
                offset_for_non_ref_pic,
                offset_for_top_to_bottom_field,
                offset_for_ref_frame,
            })
        }
        2 => Ok(PicOrderCnt::Type2),
        _ => Err(SpsError::Invalid("pic_order_cnt_type")),
    }
}

fn read_scaling_list(r: &mut BitReader<'_>, size: usize) -> Result<Vec<i32>, SpsError> {
    let mut delta_scale = Vec::new();
    let (mut last_scale, mut next_scale) = (8i64, 8i64);
    for _ in 0..size {
        if next_scale != 0 {
            let delta = r.se()?;
            delta_scale.push(delta);
            next_scale = (last_scale + i64::from(delta) + 256).rem_euclid(256);
        }
        if next_scale != 0 {
            last_scale = next_scale;
        }
    }
    Ok(delta_scale)
}

fn write_scaling_list(w: &mut BitWriter, size: usize, deltas: &[i32]) {
    let (mut last_scale, mut next_scale) = (8i64, 8i64);
    let mut deltas = deltas.iter().copied();
    for _ in 0..size {
        if next_scale != 0 {
            let delta_scale = deltas.next().expect("scaling list delta_scale");
            w.se(delta_scale);
            next_scale = (last_scale + i64::from(delta_scale) + 256).rem_euclid(256);
        }
        if next_scale != 0 {
            last_scale = next_scale;
        }
    }
    assert!(deltas.next().is_none(), "scaling list delta_scale count");
}

/// Rewrites the `level_idc` of every SPS in an Annex B unit in place, for
/// fault injection. Returns how many it changed.
pub fn set_level_idc(au: &mut [u8], level_idc: u8) -> usize {
    let mut changed = 0;
    let mut i = 0;
    while i + 3 < au.len() {
        let start = au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1;
        if start && au[i + 3] & 0x1f == 7 && i + 6 < au.len() {
            au[i + 6] = level_idc;
            changed += 1;
        }
        i += 1;
    }
    changed
}

/// Drops emulation-prevention bytes (00 00 03 -> 00 00).
fn unescape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &b in data {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Adds emulation-prevention bytes (00 00 0x with x <= 3 -> 00 00 03 0x).
fn escape(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 1);
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn bit(&mut self) -> Result<u32, SpsError> {
        let byte = self.data.get(self.pos / 8).ok_or(SpsError::Truncated)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Ok(u32::from(bit))
    }

    fn flag(&mut self) -> Result<bool, SpsError> {
        Ok(self.bit()? == 1)
    }

    fn u8(&mut self) -> Result<u8, SpsError> {
        Ok(self.bits(8)? as u8)
    }

    fn bits(&mut self, n: u32) -> Result<u32, SpsError> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    /// Unsigned exp-Golomb, up to 31 leading zeros.
    fn ue(&mut self) -> Result<u32, SpsError> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return Err(SpsError::Invalid("exp-Golomb code"));
            }
        }
        let rest = self.bits(zeros)?;
        Ok(((1u64 << zeros) - 1 + u64::from(rest)) as u32)
    }

    fn se(&mut self) -> Result<i32, SpsError> {
        let k = i64::from(self.ue()?);
        let v = if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) };
        Ok(v as i32)
    }
}

#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bits: usize,
}

impl BitWriter {
    fn bit(&mut self, one: bool) {
        if self.bits.is_multiple_of(8) {
            self.bytes.push(0);
        }
        if one {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.bits % 8);
        }
        self.bits += 1;
    }

    fn flag(&mut self, value: bool) {
        self.bit(value);
    }

    /// `u(n)`, most significant bit first.
    fn u(&mut self, n: u32, value: u32) {
        for shift in (0..n).rev() {
            self.flag(value >> shift & 1 == 1);
        }
    }

    /// Unsigned exp-Golomb.
    fn ue(&mut self, v: u32) {
        let code = u64::from(v) + 1;
        let len = 64 - code.leading_zeros();
        for _ in 1..len {
            self.bit(false);
        }
        for i in (0..len).rev() {
            self.bit(code >> i & 1 == 1);
        }
    }

    /// Signed exp-Golomb.
    fn se(&mut self, value: i32) {
        let code_num = if value > 0 {
            u64::from(u32::try_from(value).expect("se value")) * 2 - 1
        } else {
            u64::from(value.unsigned_abs()) * 2
        };
        let code_num = u32::try_from(code_num).expect("se code num");
        self.ue(code_num);
    }

    fn rbsp_trailing_bits(&mut self) {
        let rbsp_stop_one_bit = true;
        self.flag(rbsp_stop_one_bit);
        while !self.bits.is_multiple_of(8) {
            let rbsp_alignment_zero_bit = false;
            self.flag(rbsp_alignment_zero_bit);
        }
    }

    /// The bytes, the last one zero-padded.
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests;
