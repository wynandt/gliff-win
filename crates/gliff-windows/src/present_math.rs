//! A CPU emulation of `present.hlsl`'s `ps_yuv`, in the same f32 order, so
//! the shader's arithmetic is tested on the build machine against the upstream CPU
//! conversion (`gliff_sw::convert::i420_to_bgra`). The GPU itself is proved
//! on a real machine by the pattern check of `gwin-verify present`.

/// The HLSL source, embedded by the presenter and checked here.
pub const PRESENT_HLSL: &str = include_str!("present.hlsl");

/// gliff-sw's fixed-point scale (`convert.rs:32`, `FRAC = 13`).
pub const FIXED_ONE: f32 = 8192.0;

/// gliff-sw's inverse constants (`convert.rs:48-52`), by HLSL name.
pub const INVERSE: [(&str, i16); 5] = [
    ("KY", 9539),
    ("KRV", 14686),
    ("KGU", -1747),
    ("KGV", -4366),
    ("KBU", 17305),
];

fn k(name: &str) -> f32 {
    let (_, value) = INVERSE
        .iter()
        .find(|(n, _)| *n == name)
        .expect("a constant named in INVERSE");
    f32::from(*value) / FIXED_ONE
}

/// D3D's UNORM8 read: code / 255.
fn unorm_in(code: u8) -> f32 {
    f32::from(code) / 255.0
}

/// D3D's float-to-UNORM8 write: saturate, scale, round to nearest.
fn unorm_out(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `ps_yuv` for one pixel, as `[r, g, b]` codes.
pub fn ps_yuv(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = unorm_in(y) * 255.0 - 16.0;
    let cb = unorm_in(u) * 255.0 - 128.0;
    let cr = unorm_in(v) * 255.0 - 128.0;
    let r = k("KY") * y + k("KRV") * cr;
    let g = k("KY") * y + k("KGU") * cb + k("KGV") * cr;
    let b = k("KY") * y + k("KBU") * cb;
    [
        unorm_out(r / 255.0),
        unorm_out(g / 255.0),
        unorm_out(b / 255.0),
    ]
}

/// The `static const` line `present.hlsl` must carry for `name`.
pub fn hlsl_line(name: &str, value: i16) -> String {
    format!("static const float {name} = {value}.0 / 8192.0;")
}

#[cfg(test)]
mod tests;
