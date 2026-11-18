//! What the remote machine received, written as JSON lines to
//! `GWIN_FAKE_INPUT_LOG` (verification scaffolding only). Most input bugs
//! show on the remote side (a key left down, a lost Up, a wrong Resize), so
//! a native drive reads this file as the remote half of its evidence.
//!
//! One object per line, flushed as it is written, so a driver can read it
//! while the session runs:
//!
//! - `{"ev":"hello","keymap_bytes":N}`: the keymap's size, not its text.
//! - `{"ev":"key","code":30,"down":true}`: an evdev keycode.
//! - `{"ev":"button","code":272,"down":true}`
//! - `{"ev":"axis","axis":"vertical","value":..,"discrete":..,"stop":..}`
//! - `{"ev":"resize","width":..,"height":..,"scale":..}`
//! - `{"ev":"clipboard_offer","serial":..,"mime_types":[..],"files":N}`:
//!   the kinds on offer, never the data.
//! - `{"ev":"bye","held_keys":[..],"held_buttons":[..],"motions":N}`: what
//!   the remote side still held when the client said Bye. Both lists are
//!   empty when no key or button was left down.
//!
//! Pointer motion is counted, not logged, so the file stays small.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;

use gliff_proto::msg::Axis;
use gliff_proto::msg::ClientMsg;
use serde_json::Value;
use serde_json::json;

/// The remote side's view of the input: which keys and buttons are down.
#[derive(Default)]
pub struct Observed {
    held_keys: BTreeSet<u32>,
    held_buttons: BTreeSet<u32>,
    motions: u64,
}

impl Observed {
    /// The line for one message, or `None` for one the log does not record.
    pub fn record(&mut self, msg: &ClientMsg) -> Option<Value> {
        match msg {
            ClientMsg::Hello { keymap, .. } => {
                Some(json!({"ev": "hello", "keymap_bytes": keymap.len()}))
            }
            ClientMsg::Key { keycode, pressed } => {
                toggle(&mut self.held_keys, *keycode, *pressed);
                Some(json!({"ev": "key", "code": keycode, "down": pressed}))
            }
            ClientMsg::PointerButton { button, pressed } => {
                toggle(&mut self.held_buttons, *button, *pressed);
                Some(json!({"ev": "button", "code": button, "down": pressed}))
            }
            ClientMsg::PointerAxis {
                axis,
                value,
                discrete,
                stop,
            } => Some(json!({
                "ev": "axis",
                "axis": match axis {
                    Axis::Vertical => "vertical",
                    Axis::Horizontal => "horizontal",
                },
                "value": value,
                "discrete": discrete,
                "stop": stop,
            })),
            ClientMsg::PointerMotion { .. } => {
                self.motions += 1;
                None
            }
            ClientMsg::Resize {
                width,
                height,
                scale,
            } => Some(json!({"ev": "resize", "width": width, "height": height, "scale": scale})),
            ClientMsg::ClipboardOffer {
                serial,
                mime_types,
                files,
            } => Some(json!({
                "ev": "clipboard_offer",
                "serial": serial,
                "mime_types": mime_types,
                "files": files.len(),
            })),
            _ => None,
        }
    }

    /// The last line: what was still down when the client said Bye.
    pub fn bye(&self) -> Value {
        json!({
            "ev": "bye",
            "held_keys": self.held_keys,
            "held_buttons": self.held_buttons,
            "motions": self.motions,
        })
    }
}

fn toggle(set: &mut BTreeSet<u32>, code: u32, down: bool) {
    if down {
        set.insert(code);
    } else {
        set.remove(&code);
    }
}

/// The log file, when `GWIN_FAKE_INPUT_LOG` names one. Appends, so a
/// reconnect's second session lands after the first.
pub struct InputLog {
    file: Option<File>,
    observed: Observed,
}

impl InputLog {
    pub fn from_env() -> anyhow::Result<Self> {
        let file = match std::env::var_os("GWIN_FAKE_INPUT_LOG") {
            Some(path) => Some(File::options().create(true).append(true).open(path)?),
            None => None,
        };
        Ok(Self {
            file,
            observed: Observed::default(),
        })
    }

    pub fn record(&mut self, msg: &ClientMsg) -> anyhow::Result<()> {
        if self.file.is_none() {
            return Ok(());
        }
        match self.observed.record(msg) {
            Some(line) => self.write(&line),
            None => Ok(()),
        }
    }

    pub fn bye(&mut self) -> anyhow::Result<()> {
        if self.file.is_none() {
            return Ok(());
        }
        let line = self.observed.bye();
        self.write(&line)
    }

    fn write(&mut self, line: &Value) -> anyhow::Result<()> {
        if let Some(file) = &mut self.file {
            writeln!(file, "{line}")?;
            file.flush()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(keycode: u32, pressed: bool) -> ClientMsg {
        ClientMsg::Key { keycode, pressed }
    }

    #[test]
    fn a_key_let_go_is_not_held_at_bye() {
        let mut seen = Observed::default();
        let down = seen.record(&key(34, true)).expect("a key line");
        assert_eq!(down, json!({"ev": "key", "code": 34, "down": true}));
        seen.record(&key(34, false));
        assert_eq!(seen.bye()["held_keys"], json!([]));
    }

    #[test]
    fn a_key_never_let_go_is_held_at_bye() {
        let mut seen = Observed::default();
        seen.record(&key(29, true));
        seen.record(&key(34, true));
        seen.record(&key(34, false));
        seen.record(&ClientMsg::PointerButton {
            button: 0x110,
            pressed: true,
        });
        let bye = seen.bye();
        assert_eq!(bye["held_keys"], json!([29]));
        assert_eq!(bye["held_buttons"], json!([0x110]));
    }

    #[test]
    fn motion_is_counted_not_logged() {
        let mut seen = Observed::default();
        assert!(
            seen.record(&ClientMsg::PointerMotion { x: 1.0, y: 2.0 })
                .is_none()
        );
        assert_eq!(seen.bye()["motions"], json!(1));
    }

    #[test]
    fn the_hello_line_has_the_keymap_size_not_its_text() {
        let mut seen = Observed::default();
        let line = seen
            .record(&ClientMsg::Hello {
                version: 1,
                keymap: "xkb_keymap { secret }".into(),
                caps: gliff_proto::msg::ClientCaps {
                    codecs: Vec::new(),
                    max_width: 1920,
                    max_height: 1080,
                    chroma: Vec::new(),
                    features: Vec::new(),
                },
            })
            .expect("a hello line");
        assert_eq!(line, json!({"ev": "hello", "keymap_bytes": 21}));
    }
}
