//! Just enough H.264 to report what a stream signals: Annex B NAL
//! splitting and the SPS header fields (ITU-T H.264 §7.3.2.1.1).

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
    let mut r = BitReader::new(&rbsp);
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
    let _max_num_ref_frames = r.ue()?;
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
    Ok(SpsInfo {
        profile_idc,
        constraint_flags,
        level_idc,
        width: coded_w - crop_w,
        height: coded_h - crop_h,
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

#[cfg(test)]
mod tests;
