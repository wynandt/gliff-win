//! The tab strip's window: a child of the main window that paints the
//! strip with GDI and turns mouse messages into [`Action`]s. What the strip
//! looks like is [`crate::strip::draw`]; where things are is
//! [`crate::strip`]; what a click does is [`crate::strip::ui`]. This file
//! only carries them to and from Win32.
//!
//! The menus run outside the application's borrow: a popup menu runs its
//! own message loop, and the sessions' events must keep flowing while one
//! is open.

use std::time::Instant;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::GlobalFree;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::POINT;
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::Graphics::Gdi::BeginPaint;
use windows::Win32::Graphics::Gdi::BitBlt;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::Graphics::Gdi::CreateSolidBrush;
use windows::Win32::Graphics::Gdi::DeleteObject;
use windows::Win32::Graphics::Gdi::EndPaint;
use windows::Win32::Graphics::Gdi::GetDC;
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Graphics::Gdi::PAINTSTRUCT;
use windows::Win32::Graphics::Gdi::ReleaseDC;
use windows::Win32::Graphics::Gdi::SRCCOPY;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::Graphics::Gdi::SetBkColor;
use windows::Win32::Graphics::Gdi::SetTextColor;
use windows::Win32::System::DataExchange::CloseClipboard;
use windows::Win32::System::DataExchange::EmptyClipboard;
use windows::Win32::System::DataExchange::OpenClipboard;
use windows::Win32::System::DataExchange::SetClipboardData;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::GMEM_MOVEABLE;
use windows::Win32::System::Memory::GlobalAlloc;
use windows::Win32::System::Memory::GlobalLock;
use windows::Win32::System::Memory::GlobalUnlock;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT;
use windows::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent;
use windows::Win32::UI::WindowsAndMessaging::AppendMenuW;
use windows::Win32::UI::WindowsAndMessaging::CS_DBLCLKS;
use windows::Win32::UI::WindowsAndMessaging::CheckMenuRadioItem;
use windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu;
use windows::Win32::UI::WindowsAndMessaging::CreateWindowExW;
use windows::Win32::UI::WindowsAndMessaging::DefWindowProcW;
use windows::Win32::UI::WindowsAndMessaging::DestroyMenu;
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::Win32::UI::WindowsAndMessaging::GetPropW;
use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
use windows::Win32::UI::WindowsAndMessaging::HMENU;
use windows::Win32::UI::WindowsAndMessaging::HWND_TOP;
use windows::Win32::UI::WindowsAndMessaging::IDC_ARROW;
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
use windows::Win32::UI::WindowsAndMessaging::KillTimer;
use windows::Win32::UI::WindowsAndMessaging::LoadCursorW;
use windows::Win32::UI::WindowsAndMessaging::MF_BYPOSITION;
use windows::Win32::UI::WindowsAndMessaging::MF_CHECKED;
use windows::Win32::UI::WindowsAndMessaging::MF_GRAYED;
use windows::Win32::UI::WindowsAndMessaging::MF_POPUP;
use windows::Win32::UI::WindowsAndMessaging::MF_SEPARATOR;
use windows::Win32::UI::WindowsAndMessaging::MF_STRING;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::RegisterClassExW;
use windows::Win32::UI::WindowsAndMessaging::RemovePropW;
use windows::Win32::UI::WindowsAndMessaging::SPI_GETCLIENTAREAANIMATION;
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE;
use windows::Win32::UI::WindowsAndMessaging::SWP_NOZORDER;
use windows::Win32::UI::WindowsAndMessaging::SWP_SHOWWINDOW;
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::Win32::UI::WindowsAndMessaging::SetPropW;
use windows::Win32::UI::WindowsAndMessaging::SetTimer;
use windows::Win32::UI::WindowsAndMessaging::SetWindowPos;
use windows::Win32::UI::WindowsAndMessaging::ShowWindow;
use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
use windows::Win32::UI::WindowsAndMessaging::TPM_LEFTALIGN;
use windows::Win32::UI::WindowsAndMessaging::TPM_RETURNCMD;
use windows::Win32::UI::WindowsAndMessaging::TPM_RIGHTBUTTON;
use windows::Win32::UI::WindowsAndMessaging::TPM_TOPALIGN;
use windows::Win32::UI::WindowsAndMessaging::TrackPopupMenuEx;
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
use windows::Win32::UI::WindowsAndMessaging::WM_CAPTURECHANGED;
use windows::Win32::UI::WindowsAndMessaging::WM_COMMAND;
use windows::Win32::UI::WindowsAndMessaging::WM_CTLCOLOREDIT;
use windows::Win32::UI::WindowsAndMessaging::WM_ERASEBKGND;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL;
use windows::Win32::UI::WindowsAndMessaging::WM_PAINT;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_SETFONT;
use windows::Win32::UI::WindowsAndMessaging::WM_TIMER;
use windows::Win32::UI::WindowsAndMessaging::WNDCLASSEXW;
use windows::Win32::UI::WindowsAndMessaging::WS_CHILD;
use windows::Win32::UI::WindowsAndMessaging::WS_CLIPCHILDREN;
use windows::Win32::UI::WindowsAndMessaging::WS_CLIPSIBLINGS;
use windows::Win32::UI::WindowsAndMessaging::WS_VISIBLE;
use windows::core::PCWSTR;
use windows::core::w;

use crate::change::Last;
use crate::palette::Palette;
use crate::strip::DotState;
use crate::strip::Layout;
use crate::strip::Metrics;
use crate::strip::TabItem;
use crate::strip::address::Address;
use crate::strip::address::Event;
use crate::strip::dot_of;
use crate::strip::draw::BarPaint;
use crate::strip::draw::Paint;
use crate::strip::draw::TabPaint;
use crate::strip::draw::draw;
use crate::strip::edit_target;
use crate::strip::layout;
use crate::strip::menu::Command;
use crate::strip::menu::Entry;
use crate::strip::menu::ListRow;
use crate::strip::menu::Mark;
use crate::strip::menu::RecentChoice;
use crate::strip::menu::RecentRow;
use crate::strip::reveal::Pointer;
use crate::strip::reveal::Reveal;
use crate::strip::reveal::Rule;
use crate::strip::reveal::Slide;
use crate::strip::ui::Action;
use crate::strip::ui::Button;
use crate::strip::ui::Input;
use crate::strip::ui::Ui;
use crate::strip_gdi::DcMeasure;
use crate::strip_gdi::Fonts;
use crate::strip_gdi::Surface;
use crate::tabs::TabId;
use crate::tabs::Tabs;
use crate::window::with_app;

/// The window class of the strip (the main window's is another).
pub const CLASS: PCWSTR = w!("gliff-windows-strip");

/// The timer that turns the connecting arc.
const TIMER_SPIN: usize = 1;
/// One step of the arc.
const SPIN_MS: u32 = 100;
/// The timer that moves the strip in full screen and watches the pointer.
const TIMER_REVEAL: usize = 2;
/// One step of the slide (about 60 a second).
const REVEAL_MS: u32 = 16;
/// The window property that marks the strip as an overlay: it does not take
/// room from the picture.
const OVERLAY_PROP: PCWSTR = w!("gliff-windows-strip-overlay");

/// Posted to the strip to run what [`Later`] holds, once the window
/// state is free.
pub const WM_APP_DEFER: u32 = WM_APP + 20;

/// `EN_CHANGE`, in the high word of the `WM_COMMAND` the edit field sends.
const EN_CHANGE: usize = 0x0300;

/// A keyboard change the strip makes, never from inside the window state:
/// the messages it sends (`WM_SETFOCUS` and the like) would find the state
/// busy and be lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Later {
    /// Give the edit field the keyboard.
    Focus,
    /// Put this text in the edit field, the caret at its end.
    SetText(String),
    /// Give the keyboard back to the picture window, if the edit field (or
    /// nothing) has it.
    FocusMain,
}

/// What the strip asks the window to put up after the borrow is released.
pub enum Popup {
    /// The recent hosts, under the list button.
    Recent,
    /// The right-click menu of a tab, at a client point.
    Tab(TabId, (i32, i32)),
    /// The chevron's list of every tab.
    List,
}

/// The strip child window and what it draws from.
pub struct StripWin {
    hwnd: HWND,
    parent: HWND,
    metrics: Metrics,
    fonts: Fonts,
    palette: Palette,
    items: Vec<TabPaint>,
    shown: Option<TabId>,
    layout: Layout,
    scroll: i32,
    width: i32,
    visible: bool,
    pub ui: Ui,
    tracking: bool,
    /// Windows asks for no animation in windows: the arc stays still.
    animate: bool,
    spinning: bool,
    epoch: Instant,
    /// The address bar's state; its edit field is `edit`.
    pub address: Address,
    edit: HWND,
    brush: HBRUSH,
    queue: Vec<Later>,
    /// Full screen: the strip is an overlay that comes out at the top edge.
    overlay: bool,
    reveal: Reveal,
    slide: Slide,
    /// A menu of the strip is open: the strip stays out.
    menu_open: bool,
    /// The address bar is open: the strip stays out.
    bar_open: bool,
    revealing: bool,
    /// What the last layout was made from: the same again changes nothing,
    /// and is not laid out or repainted (the window syncs the strip on every
    /// pass of the message loop).
    synced: Last<SyncKey>,
    /// Where the edit field was put last.
    edit_at: Last<Option<crate::strip::Rect>>,
}

/// Everything a layout and a repaint of the strip depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
struct SyncKey {
    dpi: u32,
    fullscreen: bool,
    on_screen: bool,
    width: i32,
    items: Vec<TabPaint>,
    shown: Option<TabId>,
    bar_open: bool,
    bar_text: String,
    typing: bool,
    error: bool,
}

/// Creates the strip under `parent`, at the strip height of `dpi`.
pub fn create(parent: HWND, dpi: u32) -> windows::core::Result<StripWin> {
    // SAFETY: plain Win32 setup with static strings; the window procedure is
    // ours and the parent is ours.
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }
        let metrics = Metrics::at(dpi);
        let mut client = RECT::default();
        GetClientRect(parent, &mut client)?;
        let hwnd = CreateWindowExW(
            Default::default(),
            CLASS,
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
            0,
            0,
            client.right - client.left,
            metrics.strip_height,
            Some(parent),
            None,
            Some(instance.into()),
            None,
        )?;
        let mut animate = windows::core::BOOL(1);
        let _ = SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(std::ptr::addr_of_mut!(animate).cast()),
            Default::default(),
        );
        let fonts = Fonts::new();
        let edit = crate::strip_edit::create(hwnd, fonts.get(metrics.text, false))?;
        let brush = CreateSolidBrush(COLORREF(Palette::DARK.field.colorref()));
        let (rule, slide) = (
            Rule::at(dpi, metrics.strip_height),
            Slide::new(metrics.strip_height),
        );
        Ok(StripWin {
            hwnd,
            parent,
            fonts,
            metrics,
            palette: Palette::DARK,
            items: Vec::new(),
            shown: None,
            layout: Layout::default(),
            scroll: 0,
            width: client.right - client.left,
            visible: true,
            ui: Ui::default(),
            tracking: false,
            animate: animate.as_bool(),
            spinning: false,
            epoch: Instant::now(),
            address: Address::new(),
            edit,
            brush,
            queue: Vec::new(),
            overlay: false,
            reveal: Reveal::new(rule),
            slide,
            menu_open: false,
            bar_open: false,
            revealing: false,
            synced: Last::new(),
            edit_at: Last::new(),
        })
    }
}

/// The height of the strip under `parent` now: 0 when it is hidden, absent
/// or an overlay (full screen). The picture area is the client area below it.
pub fn height_of(parent: HWND) -> u32 {
    // SAFETY: looks for a child of our own window by class.
    unsafe {
        let Ok(child) = FindWindowExW(Some(parent), None, CLASS, None) else {
            return 0;
        };
        if !IsWindowVisible(child).as_bool() {
            return 0;
        }
        // In full screen the strip is over the picture, not above it.
        if !GetPropW(child, OVERLAY_PROP).is_invalid() {
            return 0;
        }
        let mut r = RECT::default();
        if GetWindowRect(child, &mut r).is_err() {
            return 0;
        }
        u32::try_from(r.bottom - r.top).unwrap_or(0)
    }
}

impl StripWin {
    /// The strip's height at the current scale, 0 while hidden.
    pub fn height(&self) -> i32 {
        if self.visible {
            self.metrics.strip_height
        } else {
            0
        }
    }

    fn now_ms(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    /// Whether any of the strip shows: always when docked, and in full
    /// screen while it is out or moving.
    fn on_screen(&self) -> bool {
        !self.overlay || self.slide.top(self.now_ms()) > -self.metrics.strip_height
    }

    /// Puts the child where the layout or the slide says, and shows or hides
    /// it.
    fn place(&self) {
        let (w, h) = (self.width, self.metrics.strip_height);
        // SAFETY: our own child window.
        unsafe {
            if !self.overlay {
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    w,
                    h,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else if self.on_screen() {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOP),
                    0,
                    self.slide.top(self.now_ms()),
                    w,
                    h,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Runs the timer while the strip is out or moving in full screen: it
    /// moves the slide and reads the pointer, which may have left without a
    /// message to say so.
    fn watch(&mut self) {
        let wanted = self.overlay && (self.reveal.is_out() || !self.slide.at_rest(self.now_ms()));
        if wanted == self.revealing {
            return;
        }
        self.revealing = wanted;
        // SAFETY: a timer on our own child window.
        unsafe {
            if wanted {
                SetTimer(Some(self.hwnd), TIMER_REVEAL, REVEAL_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_REVEAL);
            }
        }
    }

    /// The rule said out or hidden: the slide follows.
    fn moved(&mut self) {
        let out = self.reveal.is_out();
        if self.animate {
            self.slide.retarget(out, self.now_ms());
        } else {
            self.slide.snap(out);
        }
        self.place();
        self.watch();
    }

    /// The pointer moved over the main window (client coordinates). Only
    /// full screen cares.
    pub fn pointer(&mut self, pointer: Pointer) {
        if self.overlay && self.reveal.pointer(pointer) {
            self.moved();
        }
    }

    /// A menu of the strip opens or closes: it stays out while one is open.
    pub fn hold_menu(&mut self, open: bool) {
        self.menu_open = open;
        self.sync_hold();
    }

    /// The address bar opens or closes: the strip stays out while it is open.
    pub fn hold_bar(&mut self, open: bool) {
        self.bar_open = open;
        self.sync_hold();
    }

    fn sync_hold(&mut self) {
        if self.overlay && self.reveal.hold(self.menu_open || self.bar_open) {
            self.moved();
        }
    }

    /// One step of the timer: reads the pointer and moves the slide.
    pub fn tick(&mut self) {
        if self.overlay {
            let mut at = POINT::default();
            let mut client = RECT::default();
            // SAFETY: live structs, our parent window.
            let pointer = unsafe {
                if GetCursorPos(&mut at).is_ok()
                    && ScreenToClient(self.parent, &mut at).as_bool()
                    && GetClientRect(self.parent, &mut client).is_ok()
                    && at.x >= 0
                    && at.x < client.right
                    && at.y >= 0
                    && at.y < client.bottom
                {
                    Pointer::At(at.y)
                } else {
                    Pointer::Away
                }
            };
            if self.reveal.pointer(pointer) {
                self.moved();
                return;
            }
            self.place();
        }
        self.watch();
    }

    /// Fits the strip to the parent's width and `dpi`, and draws the tabs
    /// again. In full screen (`fullscreen`) the strip leaves the layout and
    /// waits at the top edge; see `strip::reveal`.
    pub fn sync(&mut self, tabs: &Tabs, dpi: u32, fullscreen: bool) {
        if dpi != self.metrics.dpi {
            self.metrics = Metrics::at(dpi);
            let font = self.fonts.get(self.metrics.text, false);
            // SAFETY: our own child; the font lives as long as `fonts`.
            unsafe {
                SendMessageW(
                    self.edit,
                    WM_SETFONT,
                    Some(WPARAM(font.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
        }
        let mut client = RECT::default();
        // SAFETY: our parent window and a live RECT.
        let _ = unsafe { GetClientRect(self.parent, &mut client) };
        self.width = client.right - client.left;
        let items: Vec<TabPaint> = tabs
            .iter()
            .map(|tab| TabPaint {
                id: tab.id(),
                label: tab.target().to_string(),
                dot: dot_of(tab.status(), tab.failure()),
            })
            .collect();
        let key = SyncKey {
            dpi,
            fullscreen,
            on_screen: self.on_screen(),
            width: self.width,
            items: items.clone(),
            shown: tabs.shown(),
            bar_open: self.address.is_open(),
            bar_text: self.address.text().to_owned(),
            typing: self.address.typing(),
            error: self.address.error().is_some(),
        };
        if !self.synced.update(key) {
            return;
        }
        let height = self.metrics.strip_height;
        self.reveal.set_rule(Rule::at(dpi, height));
        self.slide.set_height(height);
        if fullscreen != self.overlay {
            self.overlay = fullscreen;
            self.reveal.reset();
            self.slide.snap(false);
            self.menu_open = false;
            self.bar_open = false;
            // SAFETY: a property on our own window.
            unsafe {
                if fullscreen {
                    let _ = SetPropW(
                        self.hwnd,
                        OVERLAY_PROP,
                        Some(HANDLE(std::ptr::dangling_mut())),
                    );
                } else {
                    let _ = RemovePropW(self.hwnd, OVERLAY_PROP);
                }
            }
        }
        self.visible = !fullscreen;
        self.place();
        self.watch();
        let previous = self.shown;
        self.items = items;
        self.shown = tabs.shown();
        self.relayout();
        if self.shown != previous
            && let Some(id) = self.shown
        {
            self.scroll = self.layout.scroll_to_show(id);
            self.relayout();
        }
        let connecting = self.items.iter().any(|t| t.dot == DotState::Connecting);
        self.set_spinning(connecting && self.animate && self.on_screen());
        self.invalidate();
    }

    fn set_spinning(&mut self, on: bool) {
        if on == self.spinning {
            return;
        }
        self.spinning = on;
        // SAFETY: a timer on our own child window.
        unsafe {
            if on {
                SetTimer(Some(self.hwnd), TIMER_SPIN, SPIN_MS, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), TIMER_SPIN);
            }
        }
    }

    fn relayout(&mut self) {
        let items: Vec<TabItem> = self
            .items
            .iter()
            .map(|t| TabItem {
                id: t.id,
                label: t.label.clone(),
            })
            .collect();
        // SAFETY: a DC of our own window, released below.
        let hdc = unsafe { GetDC(Some(self.hwnd)) };
        let measure = DcMeasure {
            hdc,
            fonts: &self.fonts,
        };
        self.layout = layout(
            &self.metrics,
            self.width,
            &items,
            self.scroll,
            self.address.is_open(),
            &measure,
        );
        self.scroll = self.layout.scroll;
        // SAFETY: the DC from above.
        unsafe { ReleaseDC(Some(self.hwnd), hdc) };
        self.place_edit();
    }

    /// Puts the edit field over the bar's field, or hides it when the bar is
    /// folded (and then asks for the keyboard to go back to the picture).
    fn place_edit(&mut self) {
        let target = edit_target(self.layout.bar.map(|bar| bar.edit), self.visible);
        // Moving and showing the field again on every layout makes it blink.
        if !self.edit_at.update(target) {
            return;
        }
        // SAFETY: our own child window.
        unsafe {
            match target {
                Some(field) => {
                    let _ = SetWindowPos(
                        self.edit,
                        None,
                        field.left,
                        field.top,
                        field.width(),
                        field.bottom - field.top,
                        SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                None => {
                    if IsWindowVisible(self.edit).as_bool() {
                        let _ = ShowWindow(self.edit, SW_HIDE);
                        self.defer(Later::FocusMain);
                    }
                }
            }
        }
    }

    /// Queues a keyboard change and posts the strip a message to make it.
    pub fn defer(&mut self, item: Later) {
        self.queue.push(item);
        // SAFETY: a posted message to our own window.
        let _ = unsafe { PostMessageW(Some(self.hwnd), WM_APP_DEFER, WPARAM(0), LPARAM(0)) };
    }

    /// Where the recent-hosts menu opens: under the list button.
    pub fn list_anchor(&self) -> Option<(i32, i32)> {
        let bar = self.layout.bar?;
        Some(self.to_screen((bar.list.left, bar.list.bottom)))
    }

    pub fn invalidate(&self) {
        // SAFETY: our own window; None invalidates all of it.
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Paints the strip: the draw list on an off-screen bitmap, then one
    /// copy, so nothing flickers.
    pub fn paint(&self) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: our window, paired with EndPaint; every GDI object made
        // here is deleted before return.
        unsafe {
            let hdc = BeginPaint(self.hwnd, &mut ps);
            let (w, h) = (self.width.max(1), self.metrics.strip_height);
            let phase = if self.spinning {
                (self.epoch.elapsed().as_millis() / u128::from(SPIN_MS)) as u32
            } else {
                0
            };
            let ops = draw(&Paint {
                metrics: &self.metrics,
                palette: &self.palette,
                layout: &self.layout,
                width: self.width,
                tabs: &self.items,
                shown: self.shown,
                hover: self.ui.hover(),
                drag: self.ui.drag(&self.layout),
                bar: self.layout.bar.map(|_| BarPaint {
                    text: self.address.text(),
                    focused: self.address.typing(),
                    error: self.address.error().is_some(),
                    edit_live: self.visible,
                }),
                phase,
            });
            if let Some(mut surface) = Surface::new(Some(hdc), w, h, self.palette.strip) {
                surface.draw(&self.fonts, &ops);
                let _ = BitBlt(hdc, 0, 0, w, h, Some(surface.dc()), 0, 0, SRCCOPY);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// Feeds one input to the pointer rules and keeps the mouse capture
    /// while a button is held.
    pub fn input(&mut self, input: Input) -> Vec<Action> {
        if matches!(input, Input::Move(..)) && !self.tracking {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: asks for a WM_MOUSELEAVE on our own window.
            self.tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
        }
        if matches!(input, Input::Leave) {
            self.tracking = false;
        }
        let was_pressed = self.ui.pressed();
        let actions = self.ui.input(&self.layout, &self.metrics, input);
        // SAFETY: capture on our own window.
        unsafe {
            if self.ui.pressed() && !was_pressed {
                SetCapture(self.hwnd);
            } else if !self.ui.pressed() && was_pressed {
                let _ = ReleaseCapture();
            }
        }
        self.invalidate();
        actions
    }

    /// Moves the strip's content by `px` (a wheel notch or the chevron's
    /// scroll to a tab).
    pub fn scroll_by(&mut self, px: i32) {
        self.scroll += px;
        self.relayout();
        self.invalidate();
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Where a popup under the chevron goes: the button's bottom left, in
    /// screen coordinates.
    pub fn chevron_anchor(&self) -> Option<(i32, i32)> {
        let r = self.layout.chevron?;
        Some(self.to_screen((r.left, r.bottom)))
    }

    pub fn to_screen(&self, at: (i32, i32)) -> (i32, i32) {
        let mut p = POINT { x: at.0, y: at.1 };
        // SAFETY: our own window, a live POINT.
        let _ = unsafe { ClientToScreen(self.hwnd, &mut p) };
        (p.x, p.y)
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }
}

fn signed(lparam: LPARAM) -> (i32, i32) {
    (
        i32::from(lparam.0 as u16 as i16),
        i32::from((lparam.0 >> 16) as u16 as i16),
    )
}

impl Drop for StripWin {
    fn drop(&mut self) {
        // SAFETY: the brush this struct made; the field that uses it is a
        // child of a window that is going away.
        let _ = unsafe { DeleteObject(self.brush.into()) };
    }
}

/// The strip's window procedure.
extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    const WM_MOUSELEAVE: u32 = 0x02a3;
    let (x, y) = signed(lparam);
    let input = match msg {
        WM_PAINT => {
            if with_app(|app| app.strip.paint()).is_none() {
                // The state is busy: a plain validation, drawn at the next
                // change.
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: our window, paired with EndPaint.
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                }
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_COMMAND if (wparam.0 >> 16) & 0xffff == EN_CHANGE => {
            let edit = HWND(lparam.0 as *mut _);
            let text = crate::strip_edit::text_of(edit);
            let _ = with_app(|app| app.address_event(Event::Text(text)));
            return LRESULT(0);
        }
        WM_CTLCOLOREDIT => {
            let (text, field) = crate::strip_edit::colours(&Palette::DARK);
            let brush = with_app(|app| app.strip.brush).unwrap_or_default();
            // SAFETY: the DC the system passed for this message.
            unsafe {
                SetTextColor(HDC(wparam.0 as *mut _), COLORREF(text));
                SetBkColor(HDC(wparam.0 as *mut _), COLORREF(field));
            }
            return LRESULT(brush.0 as isize);
        }
        WM_APP_DEFER => {
            run_deferred();
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_SPIN => {
            let _ = with_app(|app| app.strip.invalidate());
            return LRESULT(0);
        }
        WM_TIMER if wparam.0 == TIMER_REVEAL => {
            let _ = with_app(|app| app.strip.tick());
            return LRESULT(0);
        }
        WM_MOUSEMOVE => Input::Move(x, y),
        WM_MOUSELEAVE => Input::Leave,
        WM_CAPTURECHANGED => Input::Cancel,
        WM_LBUTTONDOWN => Input::Down(Button::Left, x, y),
        WM_LBUTTONUP => Input::Up(Button::Left, x, y),
        WM_MBUTTONDOWN => Input::Down(Button::Middle, x, y),
        WM_MBUTTONUP => Input::Up(Button::Middle, x, y),
        WM_RBUTTONDOWN => Input::Down(Button::Right, x, y),
        WM_RBUTTONUP => Input::Up(Button::Right, x, y),
        WM_MOUSEWHEEL => Input::Wheel(i32::from((wparam.0 >> 16) as u16 as i16)),
        // SAFETY: the default handling of a message of ours.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    let popups = with_app(|app| app.strip_input(input)).unwrap_or_default();
    if !popups.is_empty() {
        let _ = with_app(|app| app.strip.hold_menu(true));
    }
    for popup in popups {
        match popup {
            Popup::Tab(id, at) => tab_menu(id, at),
            Popup::Recent => recent_menu(),
            Popup::List => chevron_menu(),
        }
        let _ = with_app(|app| app.strip.hold_menu(false));
    }
    LRESULT(0)
}

/// Builds `rows` into a popup menu and returns the chosen id (0: none).
/// `rows` are (id, text or None for a separator, checked).
fn track(hwnd: HWND, rows: &[(usize, Option<String>, bool)], at: (i32, i32)) -> usize {
    // SAFETY: a menu made and destroyed here; the strings outlive the call.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        for (id, text, checked) in rows {
            match text {
                Some(text) => {
                    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                    let flags = if *checked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    let _ = AppendMenuW(menu, flags, *id, PCWSTR(wide.as_ptr()));
                }
                None => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
            }
        }
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            at.0,
            at.1,
            hwnd,
            None,
        );
        destroy(menu);
        usize::try_from(chosen.0).unwrap_or(0)
    }
}

fn destroy(menu: HMENU) {
    // SAFETY: a menu this module made.
    let _ = unsafe { DestroyMenu(menu) };
}

fn tab_menu(id: TabId, at: (i32, i32)) {
    let Some((hwnd, screen, entries)) = with_app(|app| {
        let entries = app.tab_menu_entries(id)?;
        Some((app.strip.hwnd(), app.strip.to_screen(at), entries))
    })
    .flatten() else {
        return;
    };
    let mut commands = Vec::new();
    // SAFETY: a menu made and destroyed here; the strings outlive each call.
    let chosen = unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        build_tab(menu, &entries, &mut commands);
        let chosen = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
            screen.0,
            screen.1,
            hwnd,
            None,
        );
        // Destroying the menu destroys the submenus built into it.
        destroy(menu);
        usize::try_from(chosen.0).unwrap_or(0)
    };
    if let Some(command) = chosen.checked_sub(1).and_then(|i| commands.get(i)) {
        let command: Command = *command;
        let _ = with_app(|app| app.strip_command(command, id));
    }
}

/// Builds the rows of a tab menu into `menu`; the id of a row that does
/// something is its place in `commands` plus one. A radio row shows a dot,
/// a check row a check, a row that is not enabled and a note are greyed.
///
/// # Safety
/// `menu` is a live menu this module made.
unsafe fn build_tab(menu: HMENU, entries: &[Entry], commands: &mut Vec<Command>) {
    let wide = |text: &str| -> Vec<u16> {
        text.replace('&', "&&")
            .encode_utf16()
            .chain(Some(0))
            .collect()
    };
    // The rows that are one choice among several, by place in this menu,
    // and which of them is the choice.
    let mut radios: Vec<u32> = Vec::new();
    let mut picked: Option<u32> = None;
    // SAFETY: the caller's menu; the strings outlive each call.
    unsafe {
        for (place, entry) in (0u32..).zip(entries) {
            match entry {
                Entry::Item { command, label } => {
                    commands.push(*command);
                    let label = wide(label);
                    let _ = AppendMenuW(menu, MF_STRING, commands.len(), PCWSTR(label.as_ptr()));
                }
                Entry::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
                Entry::Note(text) => {
                    let text = wide(text);
                    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(text.as_ptr()));
                }
                Entry::Choice {
                    command,
                    label,
                    mark,
                    selected,
                    enabled,
                } => {
                    let label = wide(label);
                    let mut flags = MF_STRING;
                    let mut id = 0;
                    if *enabled {
                        commands.push(*command);
                        id = commands.len();
                    } else {
                        flags |= MF_GRAYED;
                    }
                    match mark {
                        Mark::Check if *selected => flags |= MF_CHECKED,
                        Mark::Check => {}
                        Mark::Radio => {
                            radios.push(place);
                            if *selected {
                                picked = Some(place);
                            }
                        }
                    }
                    let _ = AppendMenuW(menu, flags, id, PCWSTR(label.as_ptr()));
                }
                Entry::Submenu { label, rows } => {
                    if let Ok(sub) = CreatePopupMenu() {
                        build_tab(sub, rows, commands);
                        let label = wide(label);
                        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(label.as_ptr()));
                    }
                }
            }
        }
        if let (Some(&first), Some(&last), Some(picked)) = (radios.first(), radios.last(), picked) {
            let _ = CheckMenuRadioItem(menu, first, last, picked, MF_BYPOSITION.0);
        }
    }
}

/// Makes the queued keyboard changes, with the window state free.
fn run_deferred() {
    let Some((edit, main, items)) = with_app(|app| {
        (
            app.strip.edit,
            app.strip.parent,
            std::mem::take(&mut app.strip.queue),
        )
    }) else {
        return;
    };
    for item in items {
        match item {
            Later::Focus => {
                // SAFETY: our own child window, shown by the layout that
                // opened the bar.
                unsafe {
                    if IsWindowVisible(edit).as_bool() {
                        let _ = SetFocus(Some(edit));
                    }
                }
            }
            Later::SetText(text) => crate::strip_edit::set_text(edit, &text),
            Later::FocusMain => {
                // SAFETY: our own windows.
                unsafe {
                    let now = GetFocus();
                    if now == edit || now.0.is_null() {
                        let _ = SetFocus(Some(main));
                    }
                }
            }
        }
    }
}

/// Builds `rows` into `menu`; the id of an enabled row is its place in
/// `choices` plus one.
fn build_recent(menu: HMENU, rows: &[RecentRow], choices: &mut Vec<RecentChoice>) {
    let wide = |text: &str| -> Vec<u16> {
        text.replace('&', "&&")
            .encode_utf16()
            .chain(Some(0))
            .collect()
    };
    // SAFETY: menus made here and destroyed by the caller with their parent;
    // the strings outlive each call.
    unsafe {
        for row in rows {
            match row {
                RecentRow::Item { label, choice } => {
                    let label = wide(label);
                    match choice {
                        Some(choice) => {
                            choices.push(choice.clone());
                            let _ =
                                AppendMenuW(menu, MF_STRING, choices.len(), PCWSTR(label.as_ptr()));
                        }
                        None => {
                            let _ =
                                AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, PCWSTR(label.as_ptr()));
                        }
                    }
                }
                RecentRow::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
                RecentRow::Submenu { label, rows } => {
                    if let Ok(sub) = CreatePopupMenu() {
                        build_recent(sub, rows, choices);
                        let label = wide(label);
                        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, PCWSTR(label.as_ptr()));
                    }
                }
            }
        }
    }
}

/// The recent hosts under the list button, and what the pick does.
fn recent_menu() {
    let Some((hwnd, screen, rows)) = with_app(|app| {
        Some((
            app.strip.hwnd(),
            app.strip.list_anchor()?,
            app.recent_rows(),
        ))
    })
    .flatten() else {
        let _ = with_app(|app| app.address_event(Event::List(false)));
        return;
    };
    let mut choices = Vec::new();
    let chosen = {
        // SAFETY: a menu made and destroyed here (its submenus with it).
        unsafe {
            match CreatePopupMenu() {
                Ok(menu) => {
                    build_recent(menu, &rows, &mut choices);
                    let id = TrackPopupMenuEx(
                        menu,
                        (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LEFTALIGN | TPM_TOPALIGN).0,
                        screen.0,
                        screen.1,
                        hwnd,
                        None,
                    );
                    destroy(menu);
                    usize::try_from(id.0)
                        .ok()
                        .and_then(|id| id.checked_sub(1))
                        .and_then(|i| choices.get(i).cloned())
                }
                Err(_) => None,
            }
        }
    };
    let _ = with_app(|app| app.recent_chosen(chosen));
}

fn chevron_menu() {
    let Some((hwnd, screen, list)) = with_app(|app| {
        Some((
            app.strip.hwnd(),
            app.strip.chevron_anchor()?,
            app.chevron_rows(),
        ))
    })
    .flatten() else {
        return;
    };
    let rows: Vec<(usize, Option<String>, bool)> = list
        .iter()
        .enumerate()
        .map(|(i, row): (usize, &ListRow)| (i + 1, Some(row.label.clone()), row.checked))
        .collect();
    let chosen = track(hwnd, &rows, screen);
    if let Some(row) = chosen.checked_sub(1).and_then(|i| list.get(i)) {
        let id = row.id;
        let _ = with_app(|app| app.strip_show(id));
    }
}

/// Puts `text` on the clipboard as the user's explicit choice.
pub fn copy_text(owner: HWND, text: &str) -> windows::core::Result<()> {
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY: the clipboard is opened and closed here; the global block is
    // handed to the clipboard on success and freed otherwise; the copy
    // stays inside the block of `bytes`.
    unsafe {
        let block = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
        let ptr = GlobalLock(block);
        if ptr.is_null() {
            let _ = GlobalFree(Some(block));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), ptr.cast::<u8>(), bytes);
        let _ = GlobalUnlock(block);
        if let Err(e) = OpenClipboard(Some(owner)) {
            let _ = GlobalFree(Some(block));
            return Err(e);
        }
        let _ = EmptyClipboard();
        let result = SetClipboardData(
            CF_UNICODETEXT,
            Some(windows::Win32::Foundation::HANDLE(block.0)),
        );
        let _ = CloseClipboard();
        if let Err(e) = result {
            let _ = GlobalFree(Some(block));
            return Err(e);
        }
    }
    Ok(())
}
