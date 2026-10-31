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
            // A four-byte start code's leading zero, and trailing zero
            // bytes, belong to no unit.
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
    parse_sps_rbsp(&rbsp).map(|sps| sps.info)
}

/// The access unit with VUI bitstream restrictions (no reordering, a
/// decoded picture buffer of `max_num_ref_frames`) added to every SPS that
/// has no VUI; `None` when no SPS needed it.
///
/// Without them, a decoder must assume a POC type 0 stream may reorder,
/// and the Windows decoder then holds pictures until its whole picture
/// buffer is full. Every gliff encoder sends I and P pictures only, in
/// decode order, and writes no VUI. An SPS that has a VUI is left alone.
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
    let sps = parse_sps_rbsp(&rbsp).ok()?;
    if sps.has_vui {
        return None;
    }
    let mut w = BitWriter::default();
    for i in 0..sps.vui_flag_bit {
        w.bit((rbsp[i / 8] >> (7 - i % 8)) & 1 == 1);
    }
    w.bit(true); // vui_parameters_present_flag
    // aspect ratio, overscan, video signal, chroma location, timing, NAL
    // and VCL HRD, pic_struct: none.
    for _ in 0..8 {
        w.bit(false);
    }
    w.bit(true); // bitstream_restriction_flag
    w.bit(true); // motion_vectors_over_pic_boundaries_flag
    w.ue(2); // max_bytes_per_pic_denom: the default
    w.ue(1); // max_bits_per_mb_denom: the default
    w.ue(16); // log2_max_mv_length_horizontal: the default
    w.ue(16); // log2_max_mv_length_vertical: the default
    w.ue(0); // max_num_reorder_frames
    w.ue(sps.max_num_ref_frames.max(1)); // max_dec_frame_buffering
    w.bit(true); // rbsp_stop_one_bit
    let mut out = vec![nal[0]];
    out.extend(escape(&w.finish()));
    Some(out)
}

/// What [`restrict_sps`] needs beyond [`SpsInfo`].
struct ParsedSps {
    info: SpsInfo,
    max_num_ref_frames: u32,
    has_vui: bool,
    /// The bit offset of vui_parameters_present_flag in the RBSP.
    vui_flag_bit: usize,
}

fn parse_sps_rbsp(rbsp: &[u8]) -> Result<ParsedSps, SpsError> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.bits(8)? as u8;
    let constraint_flags = r.bits(8)? as u8;
    let level_idc = r.bits(8)? as u8;
    let _sps_id = r.ue()?;
    let mut chroma_format_idc = 1;
    if matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    ) {
        chroma_format_idc = r.ue()?;
        if chroma_format_idc > 3 {
            return Err(SpsError::Invalid("chroma_format_idc"));
        }
        if chroma_format_idc == 3 {
            let _separate_colour_plane = r.bits(1)?;
        }
        let _bit_depth_luma = r.ue()?;
        let _bit_depth_chroma = r.ue()?;
        let _qpprime_y_zero_bypass = r.bits(1)?;
        if r.bits(1)? == 1 {
            let lists = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..lists {
                if r.bits(1)? == 1 {
                    skip_scaling_list(&mut r, if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    let _log2_max_frame_num = r.ue()?;
    match r.ue()? {
        0 => {
            let _log2_max_poc_lsb = r.ue()?;
        }
        1 => {
            let _delta_always_zero = r.bits(1)?;
            let _offset_non_ref = r.se()?;
            let _offset_top_bottom = r.se()?;
            let cycle = r.ue()?;
            if cycle > 255 {
                return Err(SpsError::Invalid("num_ref_frames_in_pic_order_cnt_cycle"));
            }
            for _ in 0..cycle {
                let _offset = r.se()?;
            }
        }
        2 => {}
        _ => return Err(SpsError::Invalid("pic_order_cnt_type")),
    }
    let max_num_ref_frames = r.ue()?;
    let _gaps_allowed = r.bits(1)?;
    let width_mbs = r.ue()? + 1;
    let height_map_units = r.ue()? + 1;
    let frame_mbs_only = r.bits(1)?;
    if frame_mbs_only == 0 {
        let _mb_adaptive = r.bits(1)?;
    }
    let _direct_8x8 = r.bits(1)?;
    let (mut left, mut right, mut top, mut bottom) = (0, 0, 0, 0);
    if r.bits(1)? == 1 {
        left = r.ue()?;
        right = r.ue()?;
        top = r.ue()?;
        bottom = r.ue()?;
    }
    let (sub_w, sub_h) = match chroma_format_idc {
        1 => (2, 2),
        2 => (2, 1),
        _ => (1, 1),
    };
    let field = 2 - frame_mbs_only;
    let (crop_x, crop_y) = match chroma_format_idc {
        0 => (1, field),
        _ => (sub_w, sub_h * field),
    };
    let coded_w = width_mbs * 16;
    let coded_h = height_map_units * 16 * field;
    let crop_w = (left + right) * crop_x;
    let crop_h = (top + bottom) * crop_y;
    if crop_w >= coded_w || crop_h >= coded_h {
        return Err(SpsError::Invalid("frame cropping"));
    }
    let vui_flag_bit = r.pos;
    let has_vui = r.bits(1)? == 1;
    Ok(ParsedSps {
        info: SpsInfo {
            profile_idc,
            constraint_flags,
            level_idc,
            width: coded_w - crop_w,
            height: coded_h - crop_h,
        },
        max_num_ref_frames,
        has_vui,
        vui_flag_bit,
    })
}

/// Rewrites the `level_idc` of every SPS in an Annex B unit in place, for
/// fault injection. Returns how many it changed.
pub fn set_level_idc(au: &mut [u8], level_idc: u8) -> usize {
    let mut changed = 0;
    let mut i = 0;
    while i + 3 < au.len() {
        let start = au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1;
        // profile_idc, constraint flags and level_idc can't hold an
        // emulation-prevention byte unless two of them are zero.
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

fn skip_scaling_list(r: &mut BitReader<'_>, size: usize) -> Result<(), SpsError> {
    let (mut last, mut next) = (8i64, 8i64);
    for _ in 0..size {
        if next != 0 {
            let delta = i64::from(r.se()?);
            next = (last + delta + 256).rem_euclid(256);
        }
        if next != 0 {
            last = next;
        }
    }
    Ok(())
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

    /// The bytes, the last one zero-padded.
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests;
