//! Key names to PS/2 set 1 scancodes (what `SendInput` with
//! `KEYEVENTF_SCANCODE` takes), and the press sequence of a chord. Pure.

use gliff_windows::keymap::Prefix;
use gliff_windows::keymap::Press;
use gliff_windows::keymap::evdev;

/// A scancode: the low byte, and whether it is an `E0` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scan {
    pub code: u8,
    pub extended: bool,
}

impl Scan {
    const fn plain(code: u8) -> Self {
        Self {
            code,
            extended: false,
        }
    }

    const fn e0(code: u8) -> Self {
        Self {
            code,
            extended: true,
        }
    }
}

/// The scancode of a key name as `docs/phase4/super-chords.tsv` and the
/// gate's own sequences spell it (case-insensitive).
pub fn scan_of(name: &str) -> Option<Scan> {
    let lower = name.to_ascii_lowercase();
    let named = match lower.as_str() {
        "lwin" | "super" => Scan::e0(0x5b),
        "lctrl" | "ctrl" => Scan::plain(0x1d),
        "lshift" | "shift" => Scan::plain(0x2a),
        // The hook reports a real right Shift as scan 0x36 with the extended
        // flag (measured), so it is injected extended: the keymap forwards
        // E0 36 only since the right Shift fix, and a plain 0x36 would pass
        // without that fix.
        "rshift" => Scan::e0(0x36),
        "lalt" | "alt" => Scan::plain(0x38),
        "ralt" | "altgr" => Scan::e0(0x38),
        "enter" | "return" => Scan::plain(0x1c),
        "space" => Scan::plain(0x39),
        "tab" => Scan::plain(0x0f),
        "esc" | "escape" => Scan::plain(0x01),
        "left" => Scan::e0(0x4b),
        "home" => Scan::e0(0x47),
        "pageup" => Scan::e0(0x49),
        "pagedown" => Scan::e0(0x51),
        "delete" => Scan::e0(0x53),
        "printscreen" | "print" => Scan::e0(0x37),
        "slash" => Scan::plain(0x35),
        "period" => Scan::plain(0x34),
        "equal" => Scan::plain(0x0d),
        "f4" => Scan::plain(0x3e),
        "f10" => Scan::plain(0x44),
        "f23" => Scan::plain(0x6e),
        "f24" => Scan::plain(0x76),
        _ => return letter_or_digit(&lower),
    };
    Some(named)
}

/// Letters and digits by their place on a US keyboard, which is how a
/// scancode names a key whatever the layout.
fn letter_or_digit(lower: &str) -> Option<Scan> {
    let mut chars = lower.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    let code = match c {
        '1'..='9' => 0x02 + (c as u8 - b'1'),
        '0' => 0x0b,
        'q' => 0x10,
        'w' => 0x11,
        'e' => 0x12,
        'r' => 0x13,
        't' => 0x14,
        'y' => 0x15,
        'u' => 0x16,
        'i' => 0x17,
        'o' => 0x18,
        'p' => 0x19,
        'a' => 0x1e,
        's' => 0x1f,
        'd' => 0x20,
        'f' => 0x21,
        'g' => 0x22,
        'h' => 0x23,
        'j' => 0x24,
        'k' => 0x25,
        'l' => 0x26,
        'z' => 0x2c,
        'x' => 0x2d,
        'c' => 0x2e,
        'v' => 0x2f,
        'b' => 0x30,
        'n' => 0x31,
        'm' => 0x32,
        _ => return None,
    };
    Some(Scan::plain(code))
}

/// The presses of `LWin+LShift+S`: every key down in order, then every key
/// up in reverse. `Err` names the first key it does not know.
pub fn chord_presses(chord: &str) -> Result<Vec<(Scan, Press)>, String> {
    let keys: Vec<Scan> = chord
        .split('+')
        .map(|name| {
            scan_of(name.trim()).ok_or_else(|| format!("unknown key `{name}` in `{chord}`"))
        })
        .collect::<Result<_, _>>()?;
    let downs = keys.iter().map(|k| (*k, Press::Down));
    let ups = keys.iter().rev().map(|k| (*k, Press::Up));
    Ok(downs.chain(ups).collect())
}

/// The evdev code the client sends the remote machine for a key `SendInput` sent
/// by this scancode (what the remote machine's key tap logs), or `None` when the
/// client has no mapping. The scancode table decides, as it does in the
/// client: F24 (0x76) is the Japanese key at evdev 85 there, not KEY_F24.
pub fn evdev_of(scan: Scan) -> Option<u16> {
    let prefix = if scan.extended {
        Prefix::E0
    } else {
        Prefix::None
    };
    evdev(0, u16::from(scan.code), prefix).map(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::Press;
    use super::Scan;
    use super::chord_presses;
    use super::scan_of;

    #[test]
    fn letters_are_places_on_a_us_keyboard() {
        assert_eq!(scan_of("Y"), Some(Scan::plain(0x15)), "Y is 0x15");
        assert_eq!(
            scan_of("s"),
            Some(Scan::plain(0x1f)),
            "case does not matter"
        );
        assert_eq!(scan_of("1"), Some(Scan::plain(0x02)), "1 is 0x02");
        assert_eq!(scan_of("0"), Some(Scan::plain(0x0b)), "0 is 0x0b");
    }

    #[test]
    fn extended_keys_carry_the_e0_flag() {
        assert_eq!(scan_of("LWin"), Some(Scan::e0(0x5b)), "LWin is E0 5B");
        assert_eq!(scan_of("Left"), Some(Scan::e0(0x4b)), "Left is E0 4B");
        assert_eq!(scan_of("RAlt"), Some(Scan::e0(0x38)), "AltGr is E0 38");
        assert_eq!(scan_of("LAlt"), Some(Scan::plain(0x38)), "LAlt is 38");
    }

    #[test]
    fn right_shift_is_extended_the_way_the_hook_reports_it() {
        assert_eq!(scan_of("RShift"), Some(Scan::e0(0x36)), "E0 36");
        assert_eq!(scan_of("Slash"), Some(Scan::plain(0x35)), "plain 0x35");
        assert_eq!(
            chord_presses("RShift+Slash").expect("chord"),
            vec![
                (Scan::e0(0x36), Press::Down),
                (Scan::plain(0x35), Press::Down),
                (Scan::plain(0x35), Press::Up),
                (Scan::e0(0x36), Press::Up),
            ],
            "Shift down, slash down and up, Shift up"
        );
    }

    #[test]
    fn unknown_names_are_none() {
        assert_eq!(scan_of("Banana"), None, "not a key");
        assert_eq!(scan_of(""), None, "empty is not a key");
    }

    #[test]
    fn a_chord_presses_down_in_order_and_up_in_reverse() {
        let got = chord_presses("LWin+LShift+S").expect("chord");
        let want = vec![
            (Scan::e0(0x5b), Press::Down),
            (Scan::plain(0x2a), Press::Down),
            (Scan::plain(0x1f), Press::Down),
            (Scan::plain(0x1f), Press::Up),
            (Scan::plain(0x2a), Press::Up),
            (Scan::e0(0x5b), Press::Up),
        ];
        assert_eq!(got, want, "down in order, up in reverse");
    }

    #[test]
    fn a_chord_with_an_unknown_key_names_it() {
        let err = chord_presses("LWin+Banana").expect_err("unknown key");
        assert!(err.contains("Banana"), "the error names the key: {err}");
    }

    #[test]
    fn scancodes_map_to_the_evdev_codes_the_remote_logs() {
        use super::evdev_of;
        let ev = |name: &str| evdev_of(scan_of(name).expect("a known key"));
        assert_eq!(ev("LWin"), Some(125), "Super_L");
        assert_eq!(ev("Ctrl"), Some(29), "Control_L");
        assert_eq!(ev("Home"), Some(102), "Home");
        assert_eq!(
            ev("F24"),
            Some(85),
            "the table wins over the VK, as in the client (remote log of the first run)"
        );
        assert_eq!(ev("F10"), Some(68), "F10");
        assert_eq!(ev("RShift"), Some(54), "Shift_R");
        assert_eq!(ev("Slash"), Some(53), "slash");
        assert_eq!(ev("LShift"), Some(42), "Shift_L");
    }
}
