//! Modifier hygiene: after a grab ends, or the client does, no Shift, Ctrl,
//! Alt or Super key may stay down in Windows' own key state.
//!
//! The grab swallows keys, so Windows can miss the Up of a modifier that went
//! down before the grab (or whose Up arrived after the hook was gone) and then
//! think it is held: the next keys come out as shortcuts. The remote side is
//! handled by the session (`release_all` on every release path and on Bye);
//! this module is the local half: it asks Windows which modifiers it thinks
//! are down and injects an Up for each. The decision is pure and tested here;
//! the Win32 calls are `release_local` and `install_exit_paths` (Windows only).

/// A modifier key Windows tracks per side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifier {
    pub vk: u16,
    /// Right Ctrl and Alt and both Super keys are extended keys. The right
    /// Shift is one too in the keyboard hook (measured), but the release here
    /// is injected by virtual key, which ends the right Shift without the
    /// flag, so it is not set.
    pub extended: bool,
}

pub const VK_LSHIFT: u16 = 0xa0;
pub const VK_RSHIFT: u16 = 0xa1;
pub const VK_LCONTROL: u16 = 0xa2;
pub const VK_RCONTROL: u16 = 0xa3;
pub const VK_LMENU: u16 = 0xa4;
pub const VK_RMENU: u16 = 0xa5;
pub const VK_LWIN: u16 = 0x5b;
pub const VK_RWIN: u16 = 0x5c;

/// Every modifier, left and right.
pub const MODIFIERS: [Modifier; 8] = [
    Modifier {
        vk: VK_LSHIFT,
        extended: false,
    },
    Modifier {
        vk: VK_RSHIFT,
        extended: false,
    },
    Modifier {
        vk: VK_LCONTROL,
        extended: false,
    },
    Modifier {
        vk: VK_RCONTROL,
        extended: true,
    },
    Modifier {
        vk: VK_LMENU,
        extended: false,
    },
    Modifier {
        vk: VK_RMENU,
        extended: true,
    },
    Modifier {
        vk: VK_LWIN,
        extended: true,
    },
    Modifier {
        vk: VK_RWIN,
        extended: true,
    },
];

/// The modifiers `is_down` says Windows holds, in [`MODIFIERS`] order.
pub fn to_release(is_down: impl Fn(u16) -> bool) -> Vec<Modifier> {
    MODIFIERS
        .iter()
        .copied()
        .filter(|m| is_down(m.vk))
        .collect()
}

/// The names of `mods` for a log line, such as `LWin, LCtrl`.
pub fn names(mods: &[Modifier]) -> String {
    let name = |vk| match vk {
        VK_LSHIFT => "LShift",
        VK_RSHIFT => "RShift",
        VK_LCONTROL => "LCtrl",
        VK_RCONTROL => "RCtrl",
        VK_LMENU => "LAlt",
        VK_RMENU => "RAlt",
        VK_LWIN => "LWin",
        VK_RWIN => "RWin",
        _ => "other",
    };
    mods.iter()
        .map(|m| name(m.vk))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Whether releasing `mods` needs the Start-menu mask: a Win key Up with no
/// other key since its Down opens Start.
pub fn needs_start_mask(mods: &[Modifier]) -> bool {
    mods.iter().any(|m| matches!(m.vk, VK_LWIN | VK_RWIN))
}

/// A look at the modifiers a short while after something that may have
/// swallowed an Up: the session unlocking, the secure desktop closing. The
/// Up of a key that went down on the other desktop goes to that desktop, and
/// Windows then counts the key as down on this one. `arm` starts the wait in
/// ticks; `tick` is true once, when it ends.
#[derive(Debug, Default)]
pub struct Sweep {
    left: u8,
}

impl Sweep {
    pub fn arm(&mut self, ticks: u8) {
        self.left = ticks.max(1);
    }

    pub fn tick(&mut self) -> bool {
        match self.left {
            0 => false,
            n => {
                self.left = n - 1;
                n == 1
            }
        }
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::System::Console::CTRL_BREAK_EVENT;
    use windows::Win32::System::Console::CTRL_C_EVENT;
    use windows::Win32::System::Console::CTRL_CLOSE_EVENT;
    use windows::Win32::System::Console::CTRL_LOGOFF_EVENT;
    use windows::Win32::System::Console::CTRL_SHUTDOWN_EVENT;
    use windows::Win32::System::Console::SetConsoleCtrlHandler;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_EXTENDEDKEY;
    use windows::Win32::UI::Input::KeyboardAndMouse::KEYEVENTF_KEYUP;
    use windows::Win32::UI::Input::KeyboardAndMouse::SendInput;
    use windows::core::BOOL;

    use super::Modifier;
    use super::needs_start_mask;
    use super::to_release;

    /// Injects an Up for every modifier Windows thinks is down; returns how
    /// many. The injected events carry the client's own tag, so the hook lets
    /// them through and never forwards them.
    pub fn release_local() -> u32 {
        release_held().len() as u32
    }

    /// As [`release_local`], and says which modifiers it released.
    pub fn release_held() -> Vec<Modifier> {
        release_held_where(|_, _| true)
    }

    /// As [`release_held`], but only the modifiers `stuck(down, vk)` says are
    /// stuck: `down` is what Windows says, so a modifier it does not count as
    /// down is never touched.
    pub fn release_held_where(stuck: impl Fn(bool, u16) -> bool) -> Vec<Modifier> {
        let mods = to_release(|vk| {
            // SAFETY: GetAsyncKeyState has no preconditions.
            let down = unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
            down && stuck(down, vk)
        });
        if mods.is_empty() {
            return mods;
        }
        if needs_start_mask(&mods) {
            crate::hook::tap_mask();
        }
        let inputs: Vec<INPUT> = mods
            .iter()
            .map(|m| {
                let mut flags = KEYEVENTF_KEYUP;
                if m.extended {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                crate::hook::key_input(m.vk, flags)
            })
            .collect();
        // SAFETY: `inputs` is a valid slice of INPUT and the size is that of
        // one INPUT, as SendInput requires.
        let _ = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
        mods
    }

    /// Releases the modifiers on a panic and on the console control events,
    /// then lets the default handling go on. Call once, early.
    pub fn install_exit_paths() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            release_local();
            previous(info);
        }));
        // SAFETY: `ctrl_handler` is a valid handler routine for the process's
        // life.
        let _ = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), true) };
    }

    unsafe extern "system" fn ctrl_handler(kind: u32) -> BOOL {
        if matches!(
            kind,
            CTRL_C_EVENT
                | CTRL_BREAK_EVENT
                | CTRL_CLOSE_EVENT
                | CTRL_LOGOFF_EVENT
                | CTRL_SHUTDOWN_EVENT
        ) {
            release_local();
        }
        // Not handled: the default handler still ends the process.
        BOOL(0)
    }
}

#[cfg(windows)]
pub use win::install_exit_paths;
#[cfg(windows)]
pub use win::release_held;
#[cfg(windows)]
pub use win::release_held_where;
#[cfg(windows)]
pub use win::release_local;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sweep_fires_once_after_its_ticks() {
        let mut sweep = Sweep::default();
        assert!(!sweep.tick(), "nothing armed, nothing to do");
        sweep.arm(3);
        assert!([sweep.tick(), sweep.tick(), sweep.tick()] == [false, false, true]);
        assert!(!sweep.tick(), "once, not again");
    }

    #[test]
    fn arming_again_restarts_the_wait() {
        let mut sweep = Sweep::default();
        sweep.arm(2);
        assert!(!sweep.tick());
        sweep.arm(2);
        assert!(!sweep.tick());
        assert!(sweep.tick());
    }

    #[test]
    fn nothing_down_releases_nothing() {
        assert!(to_release(|_| false).is_empty());
    }

    #[test]
    fn every_modifier_down_is_released_left_and_right() {
        let all = to_release(|_| true);
        let vks: Vec<u16> = all.iter().map(|m| m.vk).collect();
        assert_eq!(
            vks,
            [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c],
            "L/R Shift, Ctrl, Alt, Super"
        );
    }

    #[test]
    fn only_what_windows_holds_is_released() {
        let held = to_release(|vk| vk == VK_RCONTROL || vk == VK_LSHIFT);
        assert_eq!(
            held,
            [
                Modifier {
                    vk: VK_LSHIFT,
                    extended: false
                },
                Modifier {
                    vk: VK_RCONTROL,
                    extended: true
                },
            ]
        );
    }

    #[test]
    fn a_win_key_up_gets_the_start_mask_and_the_others_do_not() {
        assert!(needs_start_mask(&to_release(|vk| vk == VK_LWIN)));
        assert!(needs_start_mask(&to_release(|vk| vk == VK_RWIN)));
        assert!(!needs_start_mask(&to_release(
            |vk| vk != VK_LWIN && vk != VK_RWIN
        )));
        assert!(!needs_start_mask(&[]));
    }

    #[test]
    fn the_log_names_each_released_modifier_in_order() {
        let held = to_release(|vk| matches!(vk, VK_LWIN | VK_RCONTROL | VK_LSHIFT));
        assert_eq!(names(&held), "LShift, RCtrl, LWin");
        assert_eq!(names(&[]), "");
    }
}
