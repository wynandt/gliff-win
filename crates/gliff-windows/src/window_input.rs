//! Keyboard, pointer and layout input on the window: the
//! window messages that carry them, turned into session
//! commands. Keys come mostly from the hook thread ([`crate::hook`]); what
//! reaches the window as a message is the mouse, focus and layout changes,
//! and keys only when the hook has gone quiet. Windows only.

use std::sync::Arc;

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::Foundation::WPARAM;
use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutNameW;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::Input::KeyboardAndMouse::SetCapture;
use windows::Win32::UI::WindowsAndMessaging::GetMessageExtraInfo;
use windows::Win32::UI::WindowsAndMessaging::GetMessageTime;
use windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMEAUTOMATIC;
use windows::Win32::UI::WindowsAndMessaging::PBT_APMRESUMESUSPEND;
use windows::Win32::UI::WindowsAndMessaging::PBT_APMSUSPEND;
use windows::Win32::UI::WindowsAndMessaging::WM_CAPTURECHANGED;
use windows::Win32::UI::WindowsAndMessaging::WM_CHAR;
use windows::Win32::UI::WindowsAndMessaging::WM_ENDSESSION;
use windows::Win32::UI::WindowsAndMessaging::WM_INPUTLANGCHANGE;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_KEYUP;
use windows::Win32::UI::WindowsAndMessaging::WM_KILLFOCUS;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_MBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEHWHEEL;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE;
use windows::Win32::UI::WindowsAndMessaging::WM_MOUSEWHEEL;
use windows::Win32::UI::WindowsAndMessaging::WM_POWERBROADCAST;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP;
use windows::Win32::UI::WindowsAndMessaging::WM_SETFOCUS;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSCHAR;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_SYSKEYUP;
use windows::Win32::UI::WindowsAndMessaging::WM_WTSSESSION_CHANGE;
use windows::Win32::UI::WindowsAndMessaging::WM_XBUTTONDOWN;
use windows::Win32::UI::WindowsAndMessaging::WM_XBUTTONUP;

use gliff_proto::msg::Axis;

use crate::grab::Grab;
use crate::grab_state::Focus;
use crate::grab_state::HookMode;
use crate::grab_state::Session;
use crate::keymap::Press;
use crate::layout::client_to_remote;
use crate::lockreg;
use crate::pointer::PointerEvent;
use crate::pointer::SideButtons;
use crate::pointer::button;
use crate::route::InputRoute;
use crate::session::Geometry;
use crate::session::UiCmd;
use crate::stats_out::GeometryInfo;
use crate::stats_out::InputInfo;
use crate::xkb::KeymapTracker;
use crate::xkb::Reason;

/// Sent when the pointer leaves the window (`TrackMouseEvent` asked for
/// it); the constant lives in a Windows module this crate does not use.
const WM_MOUSELEAVE: u32 = 0x02a3;

/// `WTS_SESSION_LOCK` and `WTS_SESSION_UNLOCK`, the `wparam` of
/// `WM_WTSSESSION_CHANGE`.
const WTS_SESSION_LOCK: usize = 0x7;
const WTS_SESSION_UNLOCK: usize = 0x8;

/// What the window knows about the picture, for mapping the pointer.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// The picture last presented.
    pub geometry: Option<Geometry>,
    /// The picture area: the client area below the tab strip.
    pub client: (u32, u32),
    /// The strip's height: the picture starts this far down.
    pub top: i32,
    pub dpi: u32,
}

pub struct WindowInput {
    hwnd: HWND,
    grab: Grab,
    cmds: Arc<InputRoute<UiCmd>>,
    keymap: KeymapTracker,
    side: SideButtons,
    /// Buttons held, for the mouse capture and to release on its loss.
    held: Vec<u32>,
    /// The layouts sent so far, for the stats file.
    keymap_log: Vec<String>,
}

impl WindowInput {
    pub fn new(
        hwnd: HWND,
        grab: Grab,
        cmds: Arc<InputRoute<UiCmd>>,
        keymap: KeymapTracker,
        side: SideButtons,
    ) -> Self {
        Self {
            hwnd,
            grab,
            cmds,
            keymap,
            side,
            held: Vec::new(),
            keymap_log: Vec::new(),
        }
    }

    pub fn grab(&self) -> &Grab {
        &self.grab
    }

    pub fn grab_mut(&mut self) -> &mut Grab {
        &mut self.grab
    }

    /// The release chord was pressed: the session has released the keys.
    pub fn grab_released(&mut self) {
        crate::logging::line("grab released by the release chord");
        self.grab.release_chord();
    }

    /// The window now shows another session and the input route points to
    /// it. The session left was told to let go of everything held
    /// (`ReleaseInput`), so this side forgets its buttons without sending
    /// Ups, and the new session is told the layout in use.
    pub fn switched(&mut self) {
        if !self.held.is_empty() {
            self.held.clear();
            // SAFETY: releases this thread's capture; fails harmlessly when
            // there is none.
            let _ = unsafe { ReleaseCapture() };
        }
        if let Some(klid) = current_klid() {
            let send = self.keymap.start(&klid);
            crate::logging::line(&send.log);
            self.note_keymap(send.log);
            self.send(UiCmd::Keymap(send.text));
        }
    }

    /// Every exit path of the window.
    pub fn shutdown(&mut self) {
        self.grab.shutdown();
        lockreg::restore_now();
    }

    /// The grab timer. A wake of the loop is not needed: it is a window
    /// timer.
    pub fn on_timer(&mut self) {
        self.grab.tick();
    }

    /// One window message. `None`: not input, or not ours to handle.
    pub fn on_message(
        &mut self,
        view: &View,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_SETFOCUS => {
                self.grab.focus(Focus::Gained);
                self.check_keymap(Reason::Focus);
                Some(LRESULT(0))
            }
            WM_KILLFOCUS => {
                self.grab.focus(Focus::Lost);
                self.release_buttons();
                Some(LRESULT(0))
            }
            // Handled, and still sent on to DefWindowProc so the system
            // switches its own layout state.
            WM_INPUTLANGCHANGE => {
                self.check_keymap(Reason::InputLang);
                None
            }
            WM_MOUSEMOVE => {
                self.grab.pointer_moved();
                self.motion(view, lparam);
                Some(LRESULT(0))
            }
            WM_MOUSELEAVE => {
                self.grab.pointer_left();
                Some(LRESULT(0))
            }
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
            | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
                self.button(view, msg, wparam, lparam);
                // WM_XBUTTON* must answer TRUE; the others answer 0.
                Some(LRESULT(isize::from(matches!(
                    msg,
                    WM_XBUTTONDOWN | WM_XBUTTONUP
                ))))
            }
            WM_MOUSEWHEEL => {
                self.wheel(Axis::Vertical, wparam);
                Some(LRESULT(0))
            }
            WM_MOUSEHWHEEL => {
                self.wheel(Axis::Horizontal, wparam);
                Some(LRESULT(0))
            }
            WM_CAPTURECHANGED => {
                self.release_buttons();
                None
            }
            WM_WTSSESSION_CHANGE => {
                match wparam.0 {
                    WTS_SESSION_LOCK => self.grab.session(Session::Locked),
                    WTS_SESSION_UNLOCK => self.grab.session(Session::Unlocked),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_ENDSESSION if wparam.0 != 0 => {
                self.shutdown();
                None
            }
            WM_POWERBROADCAST => {
                // Sleep clears the lock setting at once; the wake (either
                // kind) lets it follow the keyboard again.
                match wparam.0 as u32 {
                    PBT_APMSUSPEND => self.grab.suspend(true),
                    PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => self.grab.suspend(false),
                    _ => {}
                }
                None
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
                let press = if matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN) {
                    Press::Down
                } else {
                    Press::Up
                };
                // SAFETY: no preconditions; the time of the message being
                // handled.
                let time = unsafe { GetMessageTime() } as u32;
                // SAFETY: no preconditions. Only whether it is set is kept.
                let extra_info = unsafe { GetMessageExtraInfo() }.0 != 0;
                self.grab
                    .legacy_key(wparam.0 as u16, lparam.0, press, time, extra_info)
                    .map(|()| LRESULT(0))
            }
            // Text for keys the grab took: nothing should beep or type.
            WM_CHAR | WM_SYSCHAR if self.grab.mode() == HookMode::Grab => Some(LRESULT(0)),
            _ => None,
        }
    }

    fn send(&self, cmd: UiCmd) {
        self.cmds.send(cmd);
    }

    fn check_keymap(&mut self, reason: Reason) {
        let Some(klid) = current_klid() else {
            return;
        };
        if let Some(send) = self.keymap.check(&klid, reason) {
            crate::logging::line(&send.log);
            self.note_keymap(send.log);
            self.send(UiCmd::Keymap(send.text));
        }
    }

    /// Remembers a layout line for the stats file.
    pub fn note_keymap(&mut self, line: String) {
        const KEEP: usize = 16;
        self.keymap_log.push(line);
        if self.keymap_log.len() > KEEP {
            self.keymap_log.remove(0);
        }
    }

    fn motion(&self, view: &View, lparam: LPARAM) {
        let Some(geometry) = &view.geometry else {
            return;
        };
        let (x, y) = client_point(lparam);
        let y = y - view.top;
        if let Some((x, y)) = client_to_remote(geometry, view.client, view.dpi, (x, y)) {
            self.send(UiCmd::Pointer(PointerEvent::Motion { x, y }));
        }
    }

    fn button(&mut self, view: &View, msg: u32, wparam: WPARAM, lparam: LPARAM) {
        let xbutton = (wparam.0 >> 16) as u16;
        let Some((code, press)) = button(msg, xbutton, self.side) else {
            return;
        };
        match press {
            Press::Down => {
                self.grab.click();
                if self.held.is_empty() {
                    // SAFETY: our window; the capture keeps the Up coming
                    // here when the pointer leaves the window.
                    unsafe { SetCapture(self.hwnd) };
                }
                if !self.held.contains(&code) {
                    self.held.push(code);
                }
            }
            Press::Up => {
                self.held.retain(|held| *held != code);
                if self.held.is_empty() {
                    // SAFETY: releases this thread's capture; fails
                    // harmlessly when there is none.
                    let _ = unsafe { ReleaseCapture() };
                }
            }
        }
        // The position first, so the button lands where Windows says.
        self.motion(view, lparam);
        self.send(UiCmd::Pointer(PointerEvent::Button {
            button: code,
            press,
        }));
    }

    fn wheel(&self, axis: Axis, wparam: WPARAM) {
        let delta = (wparam.0 >> 16) as u16 as i16;
        self.send(UiCmd::Wheel { axis, delta });
    }

    /// What the stats file says about input.
    pub fn info(&self, geometry: Option<Geometry>) -> InputInfo {
        let mode = self.grab.mode();
        let counters = self.grab.counters();
        InputInfo {
            hook: match mode {
                HookMode::Off => "off",
                HookMode::SuperLOnly => "super_l_only",
                HookMode::Grab => "grab",
            }
            .to_owned(),
            grab: mode == HookMode::Grab,
            hook_reinstalls: counters.hook_reinstalls,
            legacy_keys: counters.legacy_keys,
            grab_timeouts: counters.grab_timeouts,
            modifiers_released: counters.modifiers_released,
            modifiers_released_on_exit: counters.modifiers_released_on_exit,
            lock_setting_held: self.grab.lock_held(),
            geometry: geometry.map(|g| GeometryInfo {
                stream: g.stream,
                view: g.view,
                scale_milli: g.scale_milli,
            }),
            keymap_sent: self.keymap_log.clone(),
            ..InputInfo::default()
        }
    }

    /// An Up for every button held (focus or capture lost).
    fn release_buttons(&mut self) {
        for code in std::mem::take(&mut self.held) {
            self.send(UiCmd::Pointer(PointerEvent::Button {
                button: code,
                press: Press::Up,
            }));
        }
    }
}

/// The signed client position in a mouse message's `lparam`.
fn client_point(lparam: LPARAM) -> (i32, i32) {
    let x = i32::from(lparam.0 as u16 as i16);
    let y = i32::from((lparam.0 >> 16) as u16 as i16);
    (x, y)
}

/// The active keyboard layout's KLID (`00000409`), or `None` when Windows
/// does not say.
pub fn current_klid() -> Option<String> {
    let mut name = [0u16; 9];
    // SAFETY: the buffer is the 9 wide characters (8 digits and a NUL) the
    // call documents.
    unsafe { GetKeyboardLayoutNameW(&mut name) }.ok()?;
    let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..len]))
}
