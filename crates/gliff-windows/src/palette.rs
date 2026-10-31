//! The colours the client draws itself: the tab strip, its dots, and later
//! the failure panel. One struct holds all of them, so a second theme (a light
//! one) adds a value and a choice and touches no drawing code. Only
//! the dark values exist today.
//!
//! The values come from the design mockups, which are not in this repository.
//! Translucent fills of the mockups are stored as the solid colour they
//! make over the strip, because GDI fills with solid colours.

/// An opaque colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// From `0xRRGGBB`.
    pub const fn hex(value: u32) -> Self {
        Rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    /// The `COLORREF` layout GDI takes: `0x00BBGGRR`.
    pub const fn colorref(self) -> u32 {
        (self.2 as u32) << 16 | (self.1 as u32) << 8 | self.0 as u32
    }

    /// `self` drawn over `base` with this opacity in 1/256ths, rounded to
    /// the nearest value.
    pub const fn over(self, base: Rgb, alpha: u32) -> Rgb {
        const fn mix(top: u8, below: u8, alpha: u32) -> u8 {
            ((top as u32 * alpha + below as u32 * (256 - alpha) + 128) / 256) as u8
        }
        Rgb(
            mix(self.0, base.0, alpha),
            mix(self.1, base.1, alpha),
            mix(self.2, base.2, alpha),
        )
    }
}

/// Every colour the client draws itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The strip behind the tabs.
    pub strip: Rgb,
    /// The shown tab.
    pub tab_shown: Rgb,
    /// A tab under the pointer.
    pub tab_hover: Rgb,
    /// Tab names and the strip's glyphs.
    pub text: Rgb,
    /// A name or glyph of something that is not live.
    pub text_dim: Rgb,
    /// A live tab.
    pub ok: Rgb,
    /// A tab that is trying again.
    pub warn: Rgb,
    /// A tab that failed.
    pub err: Rgb,
    /// A tab that is connecting, and the drop marker of a drag.
    pub accent: Rgb,
    /// The ring of a tab that is not connected.
    pub ring: Rgb,
    /// The window behind the picture, and behind the panel text.
    pub backdrop: Rgb,
    /// The address bar's edit field.
    pub field: Rgb,
    /// The address bar's outline.
    pub field_border: Rgb,
    /// The hint in an empty edit field.
    pub placeholder: Rgb,
    /// The address bar's go button, and under the pointer.
    pub go: Rgb,
    pub go_hover: Rgb,
    /// The failure panel's primary button, and under the pointer.
    pub primary: Rgb,
    pub primary_hover: Rgb,
}

impl Palette {
    pub const DARK: Palette = {
        let strip = Rgb::hex(0x30_30_30);
        let white = Rgb::hex(0xff_ff_ff);
        let field = Rgb::hex(0x1e_1e_1e);
        let go = Rgb::hex(0x3d_3d_3d);
        Palette {
            strip,
            // rgba(255,255,255,.14) and .07 of the mockups.
            tab_shown: white.over(strip, 36),
            tab_hover: white.over(strip, 18),
            text: white,
            // The mockups' dim label: opacity .55.
            text_dim: white.over(strip, 141),
            ok: Rgb::hex(0x8f_f0_a4),
            warn: Rgb::hex(0xf8_e4_5c),
            err: Rgb::hex(0xff_7b_63),
            accent: Rgb::hex(0x78_ae_ed),
            ring: Rgb::hex(0x9a_9a_9a),
            backdrop: Rgb::hex(0x20_20_20),
            // The mockups' #1e1e1e field, its rgba(255,255,255,.18) outline,
            // rgba(255,255,255,.4) hint and #3d3d3d button.
            field,
            field_border: white.over(strip, 46),
            placeholder: white.over(field, 102),
            go,
            go_hover: white.over(go, 18),
            // The mockups' blue button.
            primary: Rgb::hex(0x35_84_e4),
            primary_hover: white.over(Rgb::hex(0x35_84_e4), 36),
        }
    };
}

#[cfg(test)]
mod tests;
