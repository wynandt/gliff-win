//! Runs the strip's draw list ([`crate::strip::draw::Op`]) on a [`Surface`]
//! (a 32-bit DIB with a GDI device context), and measures names for the
//! layout. Shapes go through the software rasteriser
//! ([`crate::strip::raster`]) because GDI does not smooth them; names go
//! through GDI's ClearType text on the same surface. What to draw is decided
//! in [`crate::strip::draw`] and tested there.

use std::cell::RefCell;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::BI_RGB;
use windows::Win32::Graphics::Gdi::BITMAPINFO;
use windows::Win32::Graphics::Gdi::BITMAPINFOHEADER;
use windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY;
use windows::Win32::Graphics::Gdi::CreateCompatibleDC;
use windows::Win32::Graphics::Gdi::CreateDIBSection;
use windows::Win32::Graphics::Gdi::CreateFontW;
use windows::Win32::Graphics::Gdi::DEFAULT_CHARSET;
use windows::Win32::Graphics::Gdi::DIB_RGB_COLORS;
use windows::Win32::Graphics::Gdi::DT_LEFT;
use windows::Win32::Graphics::Gdi::DT_NOPREFIX;
use windows::Win32::Graphics::Gdi::DT_SINGLELINE;
use windows::Win32::Graphics::Gdi::DT_VCENTER;
use windows::Win32::Graphics::Gdi::DeleteDC;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Gdi::DrawTextW;
use windows::Win32::Graphics::Gdi::FW_BOLD;
use windows::Win32::Graphics::Gdi::FW_NORMAL;
use windows::Win32::Graphics::Gdi::GdiFlush;
use windows::Win32::Graphics::Gdi::GetTextExtentPoint32W;
use windows::Win32::Graphics::Gdi::HBITMAP;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::Graphics::Gdi::HGDIOBJ;
use windows::Win32::Graphics::Gdi::IntersectClipRect;
use windows::Win32::Graphics::Gdi::SelectClipRgn;
use windows::Win32::Graphics::Gdi::SelectObject;
use windows::Win32::Graphics::Gdi::SetBkMode;
use windows::Win32::Graphics::Gdi::SetTextColor;
use windows::Win32::Graphics::Gdi::TRANSPARENT;
use windows::core::w;

use crate::palette::Rgb;
use crate::strip::Measure;
use crate::strip::Rect;
use crate::strip::draw::Op;
use crate::strip::raster::Canvas;

fn colour(c: Rgb) -> COLORREF {
    COLORREF(c.colorref())
}

/// The names' fonts: one regular and one bold per size asked for, made when
/// first used and kept until the cache is dropped. A size is in device
/// pixels, so the cache stays valid when the scale changes.
#[derive(Default)]
pub struct Fonts {
    made: RefCell<Vec<(i32, HFONT, HFONT)>>,
}

impl Fonts {
    pub fn new() -> Self {
        Self::default()
    }

    fn make(size: i32, weight: i32) -> HFONT {
        // SAFETY: a plain font request; the name is a static string.
        unsafe {
            CreateFontW(
                -size,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                Default::default(),
                Default::default(),
                CLEARTYPE_QUALITY,
                0,
                w!("Segoe UI"),
            )
        }
    }

    /// The font of `size` pixels, regular or bold.
    pub(crate) fn get(&self, size: i32, bold: bool) -> HFONT {
        let mut made = self.made.borrow_mut();
        let at = match made.iter().position(|(s, _, _)| *s == size) {
            Some(at) => at,
            None => {
                made.push((
                    size,
                    Self::make(size, FW_NORMAL.0 as i32),
                    Self::make(size, FW_BOLD.0 as i32),
                ));
                made.len() - 1
            }
        };
        if bold { made[at].2 } else { made[at].1 }
    }

    /// The width of `text` in the bold name font of `size` pixels.
    pub fn width(&self, hdc: HDC, text: &str, size: i32) -> i32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut extent = SIZE::default();
        let font = self.get(size, true);
        // SAFETY: a live DC; the font is ours and put back before return.
        unsafe {
            let old = SelectObject(hdc, font.into());
            let _ = GetTextExtentPoint32W(hdc, &wide, &mut extent);
            SelectObject(hdc, old);
        }
        extent.cx
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        // SAFETY: fonts this struct made, not selected into any DC now.
        unsafe {
            for (_, regular, bold) in self.made.get_mut().drain(..) {
                let _ = DeleteObject(regular.into());
                let _ = DeleteObject(bold.into());
            }
        }
    }
}

/// Measures names on one DC.
pub struct DcMeasure<'a> {
    pub hdc: HDC,
    pub fonts: &'a Fonts,
}

impl Measure for DcMeasure<'_> {
    fn text_width(&self, text: &str, size: i32) -> i32 {
        self.fonts.width(self.hdc, text, size)
    }
}

fn rect(r: Rect) -> RECT {
    RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

/// A bitmap the strip is drawn on: pixels the code can reach and a device
/// context GDI can draw text on. The same surface serves the window's paint
/// and the `gwin-verify strip-shot` pictures, so both show the same pixels.
pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    width: i32,
    height: i32,
}

impl Surface {
    /// A `width` by `height` surface of the colour `fill`; `None` when
    /// Windows gives no bitmap.
    pub fn new(reference: Option<HDC>, width: i32, height: i32, fill: Rgb) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: the first row is the top.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        // SAFETY: a plain DIB request; the pointer comes back through `bits`
        // and lives as long as the bitmap, which `Drop` deletes after it
        // stops being selected. A failed request deletes the DC it made.
        let (dc, bitmap) = unsafe {
            let dc = CreateCompatibleDC(reference);
            match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => (dc, bitmap),
                Ok(bitmap) => {
                    let _ = DeleteObject(bitmap.into());
                    let _ = DeleteDC(dc);
                    return None;
                }
                Err(_) => {
                    let _ = DeleteDC(dc);
                    return None;
                }
            }
        };
        // SAFETY: selecting our own bitmap into our own DC.
        let previous = unsafe { SelectObject(dc, bitmap.into()) };
        let mut surface = Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width,
            height,
        };
        let value = u32::from(fill.0) << 16 | u32::from(fill.1) << 8 | u32::from(fill.2);
        surface.pixels_mut().fill(value);
        Some(surface)
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The pixels, `0x00RRGGBB`, row by row, after GDI's queued drawing has
    /// landed.
    pub fn pixels(&self) -> &[u32] {
        // SAFETY: GdiFlush finishes queued drawing; `bits` holds
        // `width * height` pixels for as long as `self` lives.
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts(self.bits, (self.width * self.height) as usize)
        }
    }

    fn pixels_mut(&mut self) -> &mut [u32] {
        // SAFETY: as in `pixels`; `&mut self` makes this the only reference.
        unsafe {
            let _ = GdiFlush();
            std::slice::from_raw_parts_mut(self.bits, (self.width * self.height) as usize)
        }
    }

    /// Runs `ops`: shapes in the rasteriser, names in GDI, in order, so a
    /// name lands over the shape under it. The clip is cleared at the end.
    pub fn draw(&mut self, fonts: &Fonts, ops: &[Op]) {
        let (width, height) = (self.width, self.height);
        let mut clip: Option<Op> = None;
        // SAFETY: GDI calls on our DC; every object selected is put back.
        unsafe {
            SetBkMode(self.dc, TRANSPARENT);
            let _ = SelectClipRgn(self.dc, None);
        }
        for op in ops {
            match op {
                Op::Clip(r) => {
                    clip = Some(Op::Clip(*r));
                    // SAFETY: our DC.
                    unsafe {
                        let _ = SelectClipRgn(self.dc, None);
                        IntersectClipRect(self.dc, r.left, r.top, r.right, r.bottom);
                    }
                }
                Op::Unclip => {
                    clip = None;
                    // SAFETY: our DC.
                    unsafe {
                        let _ = SelectClipRgn(self.dc, None);
                    }
                }
                Op::Text {
                    rect: r,
                    text,
                    size,
                    color,
                    bold,
                } => {
                    let font = fonts.get(*size, *bold);
                    // SAFETY: our DC; the font is selected and put back.
                    unsafe {
                        let old = SelectObject(self.dc, font.into());
                        SetTextColor(self.dc, colour(*color));
                        let mut wide: Vec<u16> = text.encode_utf16().collect();
                        let mut bounds = rect(*r);
                        DrawTextW(
                            self.dc,
                            &mut wide,
                            &mut bounds,
                            DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                        );
                        SelectObject(self.dc, old);
                    }
                }
                shape => {
                    let mut canvas = Canvas::new(self.pixels_mut(), width, height);
                    if let Some(clip) = &clip {
                        canvas.shape(clip);
                    }
                    canvas.shape(shape);
                }
            }
        }
        // SAFETY: our DC.
        unsafe {
            let _ = SelectClipRgn(self.dc, None);
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: the bitmap and DC are ours; the bitmap is unselected
        // before it is deleted.
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}
