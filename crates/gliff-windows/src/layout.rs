//! Where the picture goes in the client area: a port of upstream's
//! `paintable::layout` (gliff 089662fd `crates/gliff/src/paintable.rs:14-70`)
//! and `hypr_ipc::clean_mode` (`crates/hypr-ipc/src/lib.rs:197-209`), with
//! upstream's tests, plus the Windows ends: the device-pixel viewport, the
//! sampling filter and the one DPI-to-scale conversion, and the pointer
//! mapping `to_remote`.

use crate::session::Geometry;

/// Where the frame is drawn inside the client area, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Device pixels per view pixel.
    pub factor: f64,
}

/// A rectangle in device (physical) pixels: the D3D11 viewport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// How the blit samples the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// An integer factor (1:1 included): every device pixel is one view
    /// pixel or an exact block of them, so stay sharp.
    Point,
    /// A fractional fit, scaled linearly as GTK's `append_texture` does.
    Linear,
}

impl Layout {
    /// The integer zoom upstream asks of its decoder: the factor when the
    /// view fits, else 1.
    pub fn zoom(&self) -> u32 {
        if self.factor >= 1.0 {
            self.factor as u32
        } else {
            1
        }
    }

    /// The rectangle in device pixels. The offsets are already snapped to
    /// device pixels; the size is rounded, so a fit that is exact in
    /// device pixels stays exact.
    pub fn viewport(&self, device: f64) -> Viewport {
        let round = |v: f64| v.round().max(0.0);
        Viewport {
            x: (self.x * device).round() as i32,
            y: (self.y * device).round() as i32,
            width: round(self.width * device) as u32,
            height: round(self.height * device) as u32,
        }
    }

    /// Point at an integer factor, else linear.
    pub fn filter(&self) -> Filter {
        if self.factor.fract() == 0.0 {
            Filter::Point
        } else {
            Filter::Linear
        }
    }
}

/// A client-area point in logical pixels to the remote output's logical
/// coordinates, which is what `PointerMotion` carries (upstream's
/// `to_remote`, `gliff/src/main.rs:1336-1355`). It inverts the layout to view
/// pixels, stretches them to the coded stream size (a reduced-resolution
/// stream fills the view rectangle), clamps to the stream, and divides by the
/// stream scale. `geometry` is the picture last *presented*, not the latest
/// StreamConfig. A stream or view of zero size gives (0, 0).
pub fn to_remote(l: &Layout, geometry: &Geometry, device: f64, x: f64, y: f64) -> (f64, f64) {
    let (rw, rh) = (f64::from(geometry.stream.0), f64::from(geometry.stream.1));
    let (vw, vh) = (f64::from(geometry.view.0), f64::from(geometry.view.1));
    if rw == 0.0 || rh == 0.0 || vw == 0.0 || vh == 0.0 {
        return (0.0, 0.0);
    }
    let vx = (x - l.x) / l.factor * device;
    let vy = (y - l.y) / l.factor * device;
    let px = (vx * rw / vw).clamp(0.0, rw);
    let py = (vy * rh / vh).clamp(0.0, rh);
    let scale = (f64::from(geometry.scale_milli) / 1000.0).max(0.01);
    (px / scale, py / scale)
}

/// A mouse position in client device pixels (what `WM_MOUSEMOVE` carries) to
/// remote logical coordinates, for the picture last presented under
/// `geometry` in a client area of `client` device pixels at `dpi`. `None`
/// until a picture has been shown or while the window has no area.
pub fn client_to_remote(
    geometry: &Geometry,
    client: (u32, u32),
    dpi: u32,
    point: (i32, i32),
) -> Option<(f64, f64)> {
    let device = f64::from(dpi.max(1)) / 96.0;
    let l = layout(
        geometry.view,
        device,
        f64::from(client.0) / device,
        f64::from(client.1) / device,
    )?;
    Some(to_remote(
        &l,
        geometry,
        device,
        f64::from(point.0) / device,
        f64::from(point.1) / device,
    ))
}

/// The inverse of [`client_to_remote`]: the client-area point (device
/// pixels) that maps to a remote logical point. What the gate clicks at.
pub fn remote_to_client(
    geometry: &Geometry,
    client: (u32, u32),
    dpi: u32,
    remote: (f64, f64),
) -> Option<(i32, i32)> {
    let device = f64::from(dpi.max(1)) / 96.0;
    let l = layout(
        geometry.view,
        device,
        f64::from(client.0) / device,
        f64::from(client.1) / device,
    )?;
    let (rw, rh) = (f64::from(geometry.stream.0), f64::from(geometry.stream.1));
    let (vw, vh) = (f64::from(geometry.view.0), f64::from(geometry.view.1));
    if rw == 0.0 || rh == 0.0 {
        return None;
    }
    let scale = (f64::from(geometry.scale_milli) / 1000.0).max(0.01);
    let vx = remote.0 * scale * vw / rw;
    let vy = remote.1 * scale * vh / rh;
    let x = vx / device * l.factor + l.x;
    let y = vy / device * l.factor + l.y;
    Some(((x * device).round() as i32, (y * device).round() as i32))
}

/// Fit a `view` (device pixels) into a client area of `width` x `height`
/// logical pixels on a display of `device` scale. Offsets are snapped to
/// device pixels.
pub fn layout(view: (u32, u32), device: f64, width: f64, height: f64) -> Option<Layout> {
    let (vw, vh) = (f64::from(view.0), f64::from(view.1));
    if vw <= 0.0
        || vh <= 0.0
        || width <= 0.0
        || height <= 0.0
        || !device.is_finite()
        || device <= 0.0
    {
        return None;
    }
    let fit = (width * device / vw).min(height * device / vh);
    // The smallest even mode gives the compositor's alignment step at this
    // scale. Allow that much slack plus rounding when preserving exact
    // integer sampling; fractional scales can need more than ten device
    // pixels (upstream's comment).
    let alignment = clean_mode(2, 2, device.clamp(0.5, 4.0) as f32).0;
    let integer = fit.floor();
    let slack = (width * device - vw * integer).min(height * device - vh * integer);
    let factor = if integer >= 1.0 && slack < f64::from(alignment) + 1.0 {
        integer
    } else {
        fit
    };
    let (fw, fh) = (vw * factor / device, vh * factor / device);
    let snap = |v: f64| (v * device).floor() / device;
    Some(Layout {
        x: snap((width - fw) / 2.0),
        y: snap((height - fh) / 2.0),
        width: fw,
        height: fh,
        factor,
    })
}

/// Round a mode down until `scale` divides it into whole logical pixels
/// (upstream `hypr_ipc::clean_mode`); a scale with no small denominator is
/// left alone.
pub fn clean_mode(width: u32, height: u32, scale: f32) -> (u32, u32) {
    let s = f64::from(scale);
    let Some(q) = (1..=8u32).find(|q| {
        let numerator = s * f64::from(*q);
        (numerator - numerator.round()).abs() < 1e-4
    }) else {
        return (width, height);
    };
    let p = (s * f64::from(q)).round() as u32;
    let step = if p.is_multiple_of(2) { p } else { 2 * p };
    let fit = |v: u32| (v - v % step).max(step);
    (fit(width), fit(height))
}

/// The scale sent in `Resize` for a monitor DPI: 96 is 1.0, 192 is 2.0.
/// The one DPI conversion in the client.
pub fn protocol_scale(dpi: u32) -> f32 {
    dpi as f32 / 96.0
}

#[cfg(test)]
mod tests;
