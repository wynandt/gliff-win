//! What the user chose for one machine, and what a machine runs with once
//! the flags, the machine's choice and the file are put together.
//!
//! Pure: no Win32, no files. [`crate::config`] reads and writes the file.
//!
//! Five settings can be chosen per machine, from the tab's menu: `screen`
//! (Show as), `share_clipboard`, `sharp_colour`, `gpu_decode` and
//! `send_win_l`. A choice is kept in `config.toml` as
//! `[overrides."user@host"]`, the name the design for the settings window
//! uses, so the file does not change when that window comes. The order for a
//! machine is: a flag, the machine's choice, the `[settings]` table, the
//! default. A flag applies to every machine, so it locks the choice: the
//! menu shows it greyed and says why. `sharp_colour` and `gpu_decode` have
//! no flag and no `[settings]` key: each is the machine's choice or off.
//! `send_win_l` is locked by any `--super-l` flag (on for `registry`, off for
//! `chord` and `off`); with no choice, `super_l = "registry"` in the file
//! turns it on.

use std::cell::RefCell;
use std::rc::Rc;

use crate::cli::DesktopMode;
use crate::cli::Screen;
use crate::history::HostTarget;
use crate::input::SuperL;
use crate::session::Colour;
use crate::settings::Key;
use crate::settings::Layer;
use crate::settings::Resolved;
use crate::settings::Settings;
use crate::settings::Source;
use crate::settings::Sources;

/// What one machine's table holds: only the choices made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    pub screen: Option<Screen>,
    pub share_clipboard: Option<bool>,
    pub sharp_colour: Option<bool>,
    pub gpu_decode: Option<bool>,
    pub send_win_l: Option<bool>,
}

/// The `[overrides]` table: one [`Choice`] for each machine that has one,
/// and everything in the table that is not a choice, kept as the file had it.
#[derive(Clone, Debug, Default)]
pub struct Choices {
    machines: Vec<Machine>,
    /// Entries that are not a machine's table (a name that is not a target,
    /// or a value that is not a table).
    kept: toml::Table,
}

#[derive(Clone, Debug)]
struct Machine {
    target: HostTarget,
    choice: Choice,
    /// Keys of the machine's table that are not a choice that parsed.
    kept: toml::Table,
}

/// The choices, shared by the window (which changes them), the session
/// starter and the session set (which read them at each connect). One
/// thread uses it.
pub type ChoiceBook = Rc<RefCell<Choices>>;

impl Choices {
    /// The choices in a file's `[overrides]` table. What cannot be read is
    /// kept and named in `notes`.
    pub fn from_table(table: &toml::Table, notes: &mut Vec<String>) -> Self {
        let mut choices = Self::default();
        for (name, value) in table {
            let target = name.parse::<HostTarget>().ok();
            let (Some(target), Some(own)) = (target, value.as_table()) else {
                notes.push(format!(
                    "overrides.\"{name}\" in the file is not a user@host with a table, so it is kept as it is"
                ));
                choices.kept.insert(name.clone(), value.clone());
                continue;
            };
            let mut machine = Machine {
                target,
                choice: Choice::default(),
                kept: toml::Table::new(),
            };
            for (key, value) in own {
                match (key.as_str(), value) {
                    ("screen", toml::Value::String(text)) if text == "own" => {
                        machine.choice.screen = Some(Screen::Own);
                    }
                    ("screen", toml::Value::String(text)) if text == "mirror" => {
                        machine.choice.screen = Some(Screen::Mirror);
                    }
                    ("share_clipboard", toml::Value::Boolean(share)) => {
                        machine.choice.share_clipboard = Some(*share);
                    }
                    ("sharp_colour", toml::Value::Boolean(sharp)) => {
                        machine.choice.sharp_colour = Some(*sharp);
                    }
                    ("gpu_decode", toml::Value::Boolean(gpu)) => {
                        machine.choice.gpu_decode = Some(*gpu);
                    }
                    ("send_win_l", toml::Value::Boolean(send)) => {
                        machine.choice.send_win_l = Some(*send);
                    }
                    (
                        "screen" | "share_clipboard" | "sharp_colour" | "gpu_decode" | "send_win_l",
                        _,
                    ) => {
                        notes.push(format!(
                            "{key} in overrides.\"{name}\" is ignored: expected {}",
                            if key == "screen" {
                                "\"mirror\" or \"own\""
                            } else {
                                "true or false"
                            }
                        ));
                        machine.kept.insert(key.clone(), value.clone());
                    }
                    _ => {
                        machine.kept.insert(key.clone(), value.clone());
                    }
                }
            }
            choices.machines.push(machine);
        }
        choices
    }

    /// The machine's choices; none made is [`Choice::default`].
    pub fn of(&self, machine: &HostTarget) -> Choice {
        self.machines
            .iter()
            .find(|m| m.target.same(machine))
            .map_or_else(Choice::default, |m| m.choice)
    }

    pub fn set_screen(&mut self, machine: &HostTarget, screen: Screen) {
        self.machine(machine).choice.screen = Some(screen);
    }

    pub fn set_share_clipboard(&mut self, machine: &HostTarget, share: bool) {
        self.machine(machine).choice.share_clipboard = Some(share);
    }

    pub fn set_sharp_colour(&mut self, machine: &HostTarget, sharp: bool) {
        self.machine(machine).choice.sharp_colour = Some(sharp);
    }

    pub fn set_gpu_decode(&mut self, machine: &HostTarget, gpu: bool) {
        self.machine(machine).choice.gpu_decode = Some(gpu);
    }

    pub fn set_send_win_l(&mut self, machine: &HostTarget, send: bool) {
        self.machine(machine).choice.send_win_l = Some(send);
    }

    /// The machines that have a choice, in file order.
    pub fn machines(&self) -> Vec<(&HostTarget, Choice)> {
        self.machines
            .iter()
            .filter(|m| m.choice != Choice::default())
            .map(|m| (&m.target, m.choice))
            .collect()
    }

    /// The file form; empty when there is nothing to write.
    pub fn to_table(&self) -> toml::Table {
        let mut table = self.kept.clone();
        for machine in &self.machines {
            let mut own = machine.kept.clone();
            if let Some(screen) = machine.choice.screen {
                let word = match screen {
                    Screen::Mirror => "mirror",
                    Screen::Own => "own",
                };
                own.insert("screen".to_owned(), word.into());
            }
            if let Some(share) = machine.choice.share_clipboard {
                own.insert("share_clipboard".to_owned(), share.into());
            }
            if let Some(sharp) = machine.choice.sharp_colour {
                own.insert("sharp_colour".to_owned(), sharp.into());
            }
            if let Some(gpu) = machine.choice.gpu_decode {
                own.insert("gpu_decode".to_owned(), gpu.into());
            }
            if let Some(send) = machine.choice.send_win_l {
                own.insert("send_win_l".to_owned(), send.into());
            }
            if !own.is_empty() {
                table.insert(machine.target.as_str().to_owned(), own.into());
            }
        }
        table
    }

    fn machine(&mut self, target: &HostTarget) -> &mut Machine {
        let at = match self.machines.iter().position(|m| m.target.same(target)) {
            Some(at) => at,
            None => {
                self.machines.push(Machine {
                    target: target.clone(),
                    choice: Choice::default(),
                    kept: toml::Table::new(),
                });
                self.machines.len() - 1
            }
        };
        &mut self.machines[at]
    }
}

/// Where a machine's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Machine,
    File,
    Default,
}

impl From<Source> for Origin {
    fn from(source: Source) -> Self {
        match source {
            Source::Flag => Self::Flag,
            Source::File => Self::File,
            Source::Default => Self::Default,
        }
    }
}

impl Origin {
    /// The word `--print-settings` shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Machine => "machine",
            Self::File => "file",
            Self::Default => "default",
        }
    }
}

/// A value that no machine's choice can change, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock<T> {
    pub value: T,
    pub origin: Origin,
    /// A sentence for the menu.
    pub reason: String,
}

/// What the flags and the file fix for every machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Locks {
    pub screen: Option<Lock<Screen>>,
    pub share_clipboard: Option<Lock<bool>>,
    pub send_win_l: Option<Lock<bool>>,
}

impl Locks {
    /// From the flags' layer and the merged settings.
    pub fn new(flags: &Layer, resolved: &Resolved) -> Self {
        let screen = if flags.screen == Some(Screen::Own) {
            Some(Lock {
                value: Screen::Own,
                origin: Origin::Flag,
                reason: "Locked: this window was started with --headless, which applies to every machine.".to_owned(),
            })
        } else if matches!(flags.output, Some(Some(_))) {
            Some(Lock {
                value: Screen::Mirror,
                origin: Origin::Flag,
                reason:
                    "Locked: this window was started with --output, which applies to every machine."
                        .to_owned(),
            })
        } else {
            None
        };
        let share_clipboard = if resolved.settings.share_clipboard {
            None
        } else if flags.share_clipboard == Some(false) {
            Some(Lock {
                value: false,
                origin: Origin::Flag,
                reason: "Locked: this window was started with --no-clipboard, which turns the clipboard off for every machine.".to_owned(),
            })
        } else {
            Some(Lock {
                value: false,
                origin: Origin::File,
                reason: "Locked: config.toml has share_clipboard = false, which turns the clipboard off for every machine."
                    .to_owned(),
            })
        };
        let send_win_l = flags.super_l.map(|super_l| Lock {
            value: super_l == SuperL::Registry,
            origin: Origin::Flag,
            reason:
                "Locked: this window was started with --super-l, which applies to every machine."
                    .to_owned(),
        });
        Self {
            screen,
            share_clipboard,
            send_win_l,
        }
    }
}

/// The choices a live session started with, kept so the menu can tell
/// when the saved choice has moved on from them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Running {
    pub screen: Screen,
    pub share_clipboard: bool,
    pub sharp_colour: bool,
    pub gpu_decode: bool,
}

/// What one machine runs with, and what the menu shows for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Connection {
    pub screen: Screen,
    pub screen_origin: Origin,
    pub screen_lock: Option<String>,
    pub share_clipboard: bool,
    pub share_origin: Origin,
    pub share_lock: Option<String>,
    /// Off unless the machine's choice turns it on; nothing locks it.
    pub sharp_colour: bool,
    pub sharp_origin: Origin,
    /// Off unless the machine's choice turns it on; nothing locks it.
    pub gpu_decode: bool,
    pub gpu_origin: Origin,
    /// Whether a real Win+L goes to this machine: Windows' own Win+L is
    /// turned off (the per-user `DisableLockWorkstation` setting is set to 1)
    /// while the window shows it and has the keyboard. Live: never part of
    /// [`Running`].
    pub send_win_l: bool,
    pub send_win_l_origin: Origin,
    pub send_win_l_lock: Option<String>,
}

impl Connection {
    pub fn of(settings: &Settings, sources: &Sources, locks: &Locks, choice: Choice) -> Self {
        let from_settings = || match settings.desktop_mode() {
            DesktopMode::Headless => Screen::Own,
            DesktopMode::Mirror { .. } => Screen::Mirror,
        };
        let (screen, screen_origin, screen_lock) = match (&locks.screen, choice.screen) {
            (Some(lock), _) => (lock.value, lock.origin, Some(lock.reason.clone())),
            (None, Some(screen)) => (screen, Origin::Machine, None),
            (None, None) => {
                let key = if settings.output.is_some() {
                    Key::Output
                } else {
                    Key::Screen
                };
                (from_settings(), sources.of(key).into(), None)
            }
        };
        let (share_clipboard, share_origin, share_lock) =
            match (&locks.share_clipboard, choice.share_clipboard) {
                (Some(lock), _) => (lock.value, lock.origin, Some(lock.reason.clone())),
                (None, Some(share)) => (share, Origin::Machine, None),
                (None, None) => (
                    settings.share_clipboard,
                    sources.of(Key::ShareClipboard).into(),
                    None,
                ),
            };
        let (sharp_colour, sharp_origin) = match choice.sharp_colour {
            Some(sharp) => (sharp, Origin::Machine),
            None => (false, Origin::Default),
        };
        let (gpu_decode, gpu_origin) = match choice.gpu_decode {
            Some(gpu) => (gpu, Origin::Machine),
            None => (false, Origin::Default),
        };
        let (send_win_l, send_win_l_origin, send_win_l_lock) =
            match (&locks.send_win_l, choice.send_win_l) {
                (Some(lock), _) => (lock.value, lock.origin, Some(lock.reason.clone())),
                (None, Some(send)) => (send, Origin::Machine, None),
                (None, None) => (
                    settings.super_l == SuperL::Registry,
                    sources.of(Key::SuperL).into(),
                    None,
                ),
            };
        Self {
            screen,
            screen_origin,
            screen_lock,
            share_clipboard,
            share_origin,
            share_lock,
            sharp_colour,
            sharp_origin,
            gpu_decode,
            gpu_origin,
            send_win_l,
            send_win_l_origin,
            send_win_l_lock,
        }
    }

    /// What a session started now would run with.
    pub fn running(&self) -> Running {
        Running {
            screen: self.screen,
            share_clipboard: self.share_clipboard,
            sharp_colour: self.sharp_colour,
            gpu_decode: self.gpu_decode,
        }
    }

    /// The chroma modes a session started now offers.
    pub fn colour(&self) -> Colour {
        Colour::sharp(self.sharp_colour)
    }

    /// Whether a session that started with `live` would run differently if it
    /// started now. A choice a flag or the file has locked never counts: it
    /// cannot change from the menu.
    pub fn pending(&self, live: &Running) -> bool {
        let screen = self.screen_lock.is_none() && self.screen != live.screen;
        let share = self.share_lock.is_none() && self.share_clipboard != live.share_clipboard;
        let sharp = self.sharp_colour != live.sharp_colour;
        let gpu = self.gpu_decode != live.gpu_decode;
        screen || share || sharp || gpu
    }

    /// The `--print-settings` form: each choice with its value, where it
    /// came from, and whether a flag or the file locks it.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "screen": {
                "value": match self.screen { Screen::Mirror => "mirror", Screen::Own => "own" },
                "source": self.screen_origin.name(),
                "locked": self.screen_lock.is_some(),
            },
            "share_clipboard": {
                "value": self.share_clipboard,
                "source": self.share_origin.name(),
                "locked": self.share_lock.is_some(),
            },
            "sharp_colour": {
                "value": self.sharp_colour,
                "source": self.sharp_origin.name(),
                "locked": false,
            },
            "gpu_decode": {
                "value": self.gpu_decode,
                "source": self.gpu_origin.name(),
                "locked": false,
            },
            "send_win_l": {
                "value": self.send_win_l,
                "source": self.send_win_l_origin.name(),
                "locked": self.send_win_l_lock.is_some(),
            },
        })
    }

    /// The server's screen mode for this machine. With no choice and no
    /// lock it is exactly [`Settings::desktop_mode`].
    pub fn desktop_mode(&self, settings: &Settings) -> DesktopMode {
        match (self.screen_origin, self.screen) {
            (Origin::Machine, Screen::Own) => DesktopMode::Headless,
            (Origin::Machine, Screen::Mirror) => settings
                .output
                .clone()
                .map_or_else(DesktopMode::mirror_focused, |output| DesktopMode::Mirror {
                    output,
                }),
            _ => settings.desktop_mode(),
        }
    }
}

/// The merged settings, the locks and the book of choices: everything that
/// decides what one machine runs with. The book is read at each call, so a
/// choice made in the menu is seen by the next connect.
#[derive(Debug)]
pub struct Rules {
    resolved: Resolved,
    locks: Locks,
    book: ChoiceBook,
}

impl Rules {
    pub fn new(resolved: Resolved, locks: Locks, book: ChoiceBook) -> Self {
        Self {
            resolved,
            locks,
            book,
        }
    }

    pub fn connection(&self, machine: &HostTarget) -> Connection {
        let choice = self.book.borrow().of(machine);
        Connection::of(
            &self.resolved.settings,
            &self.resolved.sources,
            &self.locks,
            choice,
        )
    }

    pub fn desktop_mode(&self, machine: &HostTarget) -> DesktopMode {
        self.connection(machine)
            .desktop_mode(&self.resolved.settings)
    }

    pub fn shares_clipboard(&self, machine: &HostTarget) -> bool {
        self.connection(machine).share_clipboard
    }
}

#[cfg(test)]
mod tests;
