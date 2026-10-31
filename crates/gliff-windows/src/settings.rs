//! The settings: one table of keys, with a default, a change class and a
//! flag for each, and the one rule that decides which source a value comes
//! from (a flag, then the file, then the default).
//!
//! Pure: no Win32, no files. [`crate::config`] reads and writes the file;
//! this module only turns a TOML table into a [`Layer`] and layers into
//! [`Settings`].

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::ValueEnum;

use crate::cli::AdapterChoice;
use crate::cli::DEFAULT_SERVER_PATH;
use crate::cli::DesktopMode;
use crate::cli::Screen;
use crate::grab_state::Follow;
use crate::input::InjectedKeys;
use crate::input::InputOptions;
use crate::input::KeyPolicy;
use crate::input::MediaKeys;
use crate::input::SuperL;
use crate::keymap::Chord;
use crate::keymap::Evdev;
use crate::keymap::ModifierSet;
use crate::local_chords::LocalChordList;
use crate::pointer::SideButtons;

/// Where a setting's value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Default,
    File,
    Flag,
}

impl Source {
    /// The word `--print-settings` shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::File => "file",
            Self::Flag => "flag",
        }
    }
}

/// What a change to a setting needs before it takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeClass {
    /// At once, in a running client.
    Live,
    /// At the next connection.
    Reconnect,
    /// At the next start of the client.
    Restart,
}

impl ChangeClass {
    /// The word `--print-settings` shows.
    pub fn name(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Reconnect => "reconnect",
            Self::Restart => "restart",
        }
    }
}

/// An optional binding: `none` turns it off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding<T> {
    Off,
    On(T),
}

impl<T: fmt::Display> fmt::Display for Binding<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => f.write_str("none"),
            Self::On(value) => value.fmt(f),
        }
    }
}

impl<T: FromStr> FromStr for Binding<T> {
    type Err = T::Err;

    fn from_str(text: &str) -> Result<Self, T::Err> {
        if text.trim().eq_ignore_ascii_case("none") {
            Ok(Self::Off)
        } else {
            text.parse().map(Self::On)
        }
    }
}

/// The grab time limit in seconds: 1 to 86400.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrabTimeout(u32);

/// A grab timeout outside 1 to 86400, or not a number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a number of seconds from 1 to 86400")]
pub struct GrabTimeoutError(String);

impl GrabTimeout {
    const MAX: u32 = 86_400;

    pub fn new(secs: u32) -> Result<Self, GrabTimeoutError> {
        if (1..=Self::MAX).contains(&secs) {
            Ok(Self(secs))
        } else {
            Err(GrabTimeoutError(secs.to_string()))
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for GrabTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for GrabTimeout {
    type Err = GrabTimeoutError;

    fn from_str(text: &str) -> Result<Self, GrabTimeoutError> {
        text.trim()
            .parse()
            .ok()
            .and_then(|secs| Self::new(secs).ok())
            .ok_or_else(|| GrabTimeoutError(text.to_owned()))
    }
}

/// How a setting's value is written in the TOML file and in the JSON report.
/// `from_toml` is the one place that parses a file value.
pub trait Value: Sized {
    /// The value, or why the file's value is refused.
    fn from_toml(value: &toml::Value) -> Result<Self, String>;
    /// The file's form; `None` for a setting that is not set.
    fn to_toml(&self) -> Option<toml::Value>;
    fn to_json(&self) -> serde_json::Value;
}

fn text_of(value: &toml::Value) -> Result<&str, String> {
    value
        .as_str()
        .ok_or_else(|| "expected text in quotes".to_owned())
}

/// A value written as the text of its `FromStr` and `Display`.
macro_rules! text_value {
    ($($ty:ty),* $(,)?) => {$(
        impl Value for $ty {
            fn from_toml(value: &toml::Value) -> Result<Self, String> {
                text_of(value)?.parse().map_err(|e: <$ty as FromStr>::Err| e.to_string())
            }
            fn to_toml(&self) -> Option<toml::Value> {
                Some(toml::Value::String(self.to_string()))
            }
            fn to_json(&self) -> serde_json::Value {
                serde_json::Value::String(self.to_string())
            }
        }
    )*};
}

/// A value written as the name of a clap enum variant.
macro_rules! enum_value {
    ($($ty:ty),* $(,)?) => {$(
        impl Value for $ty {
            fn from_toml(value: &toml::Value) -> Result<Self, String> {
                let text = text_of(value)?;
                <$ty as ValueEnum>::from_str(text, true).map_err(|_| {
                    let names: Vec<String> = <$ty as ValueEnum>::value_variants()
                        .iter()
                        .filter_map(ValueEnum::to_possible_value)
                        .map(|v| v.get_name().to_owned())
                        .collect();
                    format!("{text:?} is not one of {}", names.join(", "))
                })
            }
            fn to_toml(&self) -> Option<toml::Value> {
                self.to_possible_value()
                    .map(|v| toml::Value::String(v.get_name().to_owned()))
            }
            fn to_json(&self) -> serde_json::Value {
                self.to_possible_value()
                    .map_or(serde_json::Value::Null, |v| {
                        serde_json::Value::String(v.get_name().to_owned())
                    })
            }
        }
    )*};
}

text_value!(Chord, ModifierSet, Binding<Chord>, Binding<ModifierSet>);
enum_value!(
    Follow,
    InjectedKeys,
    SuperL,
    SideButtons,
    AdapterChoice,
    Screen
);

impl Value for bool {
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        value
            .as_bool()
            .ok_or_else(|| "expected true or false".to_owned())
    }
    fn to_toml(&self) -> Option<toml::Value> {
        Some(toml::Value::Boolean(*self))
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Bool(*self)
    }
}

impl Value for String {
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        let text = text_of(value)?;
        if text.is_empty() {
            return Err("expected text that is not empty".to_owned());
        }
        Ok(text.to_owned())
    }
    fn to_toml(&self) -> Option<toml::Value> {
        Some(toml::Value::String(self.clone()))
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.clone())
    }
}

impl Value for PathBuf {
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        String::from_toml(value).map(Self::from)
    }
    fn to_toml(&self) -> Option<toml::Value> {
        Some(toml::Value::String(self.display().to_string()))
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.display().to_string())
    }
}

impl Value for GrabTimeout {
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        let secs = value
            .as_integer()
            .ok_or_else(|| "expected a whole number of seconds".to_owned())?;
        u32::try_from(secs)
            .map_err(|_| GrabTimeoutError(secs.to_string()))
            .and_then(Self::new)
            .map_err(|e| e.to_string())
    }
    fn to_toml(&self) -> Option<toml::Value> {
        Some(toml::Value::Integer(i64::from(self.0)))
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::from(self.0)
    }
}

impl Value for LocalChordList {
    /// Comma-separated text (the flag's form), or a list of chords.
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        match value {
            toml::Value::String(text) => text
                .parse()
                .map_err(|e: crate::local_chords::LocalChordError| e.to_string()),
            toml::Value::Array(items) => items
                .iter()
                .map(|item| {
                    text_of(item)?
                        .parse()
                        .map_err(|e: crate::local_chords::LocalChordError| e.to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Self),
            _ => Err("expected text or a list of chords".to_owned()),
        }
    }
    fn to_toml(&self) -> Option<toml::Value> {
        Some(toml::Value::String(self.to_string()))
    }
    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(self.to_string())
    }
}

/// A setting that may be unset (`grab_timeout_secs`, `output`, `log_dir`).
impl<T: Value> Value for Option<T> {
    fn from_toml(value: &toml::Value) -> Result<Self, String> {
        T::from_toml(value).map(Some)
    }
    fn to_toml(&self) -> Option<toml::Value> {
        self.as_ref().and_then(T::to_toml)
    }
    fn to_json(&self) -> serde_json::Value {
        self.as_ref().map_or(serde_json::Value::Null, T::to_json)
    }
}

/// Builds [`Settings`], [`Layer`], [`Key`] and [`Sources`] from one table, so
/// that a setting is added in one place and cannot be missing from one of them.
macro_rules! settings {
    ($(
        $variant:ident {
            field: $field:ident,
            ty: $ty:ty,
            default: $default:expr,
            class: $class:ident,
            key: $key:literal,
            flag: $flag:literal,
        }
    )*) => {
        /// The settings the client runs with.
        #[derive(Clone, Debug, PartialEq)]
        pub struct Settings {
            $(pub $field: $ty,)*
        }

        impl Default for Settings {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }

        /// Some of the settings: what one source (the flags, or the file) has
        /// a value for.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct Layer {
            $(pub $field: Option<$ty>,)*
        }

        /// One setting, by name.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Key {
            $($variant,)*
        }

        impl Key {
            pub const ALL: &'static [Key] = &[$(Key::$variant,)*];

            /// The key in `config.toml`.
            pub fn name(self) -> &'static str {
                match self {
                    $(Key::$variant => $key,)*
                }
            }

            pub fn class(self) -> ChangeClass {
                match self {
                    $(Key::$variant => ChangeClass::$class,)*
                }
            }

            /// The command-line flag, if there is one.
            pub fn flag(self) -> Option<&'static str> {
                match self {
                    $(Key::$variant => (!$flag.is_empty()).then_some($flag),)*
                }
            }
        }

        /// Where each setting's value came from.
        #[derive(Clone, Debug, PartialEq, Eq)]
        pub struct Sources {
            $($field: Source,)*
        }

        impl Sources {
            pub fn of(&self, key: Key) -> Source {
                match key {
                    $(Key::$variant => self.$field,)*
                }
            }
        }

        impl Layer {
            /// The values a file table holds. A value that does not parse is
            /// left out and named in the notes; a key that is not a setting
            /// is ignored.
            pub fn from_table(table: &toml::Table) -> (Self, Vec<String>) {
                let mut layer = Self::default();
                let mut notes = Vec::new();
                $(
                    if let Some(value) = table.get($key) {
                        match <$ty as Value>::from_toml(value) {
                            Ok(parsed) => layer.$field = Some(parsed),
                            Err(why) => notes.push(format!(
                                "{} in the file is ignored: {why}", $key
                            )),
                        }
                    }
                )*
                (layer, notes)
            }

            /// Gives one setting no value.
            pub fn clear(&mut self, key: Key) {
                match key {
                    $(Key::$variant => self.$field = None,)*
                }
            }

            /// The file form of the values this layer has.
            pub fn to_table(&self) -> toml::Table {
                let mut table = toml::Table::new();
                $(
                    if let Some(value) = self.$field.as_ref().and_then(Value::to_toml) {
                        table.insert($key.to_owned(), value);
                    }
                )*
                table
            }
        }

        impl Settings {
            /// Each setting as `{value, source, class}`, in table order.
            pub fn to_json(&self, sources: &Sources) -> serde_json::Value {
                let mut map = serde_json::Map::new();
                $(
                    map.insert($key.to_owned(), serde_json::json!({
                        "value": Value::to_json(&self.$field),
                        "source": sources.$field.name(),
                        "class": ChangeClass::$class.name(),
                    }));
                )*
                serde_json::Value::Object(map)
            }
        }

        /// Merges flag, then file, then default, one setting at a time.
        fn merge(flags: &Layer, file: &Layer) -> (Settings, Sources) {
            let defaults = Settings::default();
            let mut sources = Sources { $($field: Source::Default,)* };
            let settings = Settings {
                $(
                    $field: if let Some(value) = &flags.$field {
                        sources.$field = Source::Flag;
                        value.clone()
                    } else if let Some(value) = &file.$field {
                        sources.$field = Source::File;
                        value.clone()
                    } else {
                        defaults.$field
                    },
                )*
            };
            (settings, sources)
        }
    };
}

settings! {
    ReleaseChord {
        field: release_chord, ty: Chord, default: Chord::ctrl_alt_home(),
        class: Live, key: "release_chord", flag: "--release-chord",
    }
    GrabFollows {
        field: grab_follows, ty: Follow, default: Follow::Focus,
        class: Live, key: "grab_follows", flag: "--grab-follows",
    }
    GrabTimeoutSecs {
        field: grab_timeout_secs, ty: Option<GrabTimeout>, default: None,
        class: Live, key: "grab_timeout_secs", flag: "--grab-timeout-secs",
    }
    InjectedKeys {
        field: injected_keys, ty: InjectedKeys, default: InjectedKeys::Forward,
        class: Live, key: "injected_keys", flag: "--injected-keys",
    }
    ForwardMediaKeys {
        field: forward_media_keys, ty: bool, default: false,
        class: Live, key: "forward_media_keys", flag: "--forward-media-keys",
    }
    LocalChords {
        field: local_chords, ty: LocalChordList, default: LocalChordList::default(),
        class: Live, key: "local_chords", flag: "--local-chords",
    }
    SideButtons {
        field: side_buttons, ty: SideButtons, default: SideButtons::Upstream,
        class: Live, key: "side_buttons", flag: "--side-buttons",
    }
    SuperLChord {
        field: super_l_chord, ty: Chord, default: Chord::ctrl_alt_l(),
        class: Live, key: "super_l_chord", flag: "--super-l-chord",
    }
    SwitchNext {
        field: switch_next, ty: Binding<Chord>, default: Binding::On(Chord::ctrl_alt_page_down()),
        class: Live, key: "switch_next", flag: "",
    }
    SwitchPrevious {
        field: switch_previous, ty: Binding<Chord>, default: Binding::On(Chord::ctrl_alt_page_up()),
        class: Live, key: "switch_previous", flag: "",
    }
    SwitchJump {
        field: switch_jump, ty: Binding<ModifierSet>,
        default: Binding::On(ModifierSet::CTRL.with(ModifierSet::ALT)),
        class: Live, key: "switch_jump", flag: "",
    }
    KeepInTray {
        field: keep_in_tray, ty: bool, default: false,
        class: Live, key: "keep_in_tray", flag: "",
    }
    RememberRecentHosts {
        field: remember_recent_hosts, ty: bool, default: true,
        class: Live, key: "remember_recent_hosts", flag: "",
    }
    ClipboardHistory {
        field: clipboard_history, ty: bool, default: false,
        class: Live, key: "clipboard_history", flag: "--clipboard-history",
    }
    ShareClipboard {
        field: share_clipboard, ty: bool, default: true,
        class: Reconnect, key: "share_clipboard", flag: "--no-clipboard",
    }
    ServerPath {
        field: server_path, ty: String, default: DEFAULT_SERVER_PATH.to_owned(),
        class: Reconnect, key: "server_path", flag: "--server-path",
    }
    Ssh {
        field: ssh, ty: PathBuf, default: crate::system_paths::default_ssh(),
        class: Reconnect, key: "ssh", flag: "--ssh",
    }
    Screen {
        field: screen, ty: Screen, default: Screen::Mirror,
        class: Reconnect, key: "screen", flag: "--headless",
    }
    Output {
        field: output, ty: Option<String>, default: None,
        class: Reconnect, key: "output", flag: "--output",
    }
    SuperL {
        field: super_l, ty: SuperL, default: SuperL::Chord,
        class: Restart, key: "super_l", flag: "--super-l",
    }
    LogDir {
        field: log_dir, ty: Option<PathBuf>, default: None,
        class: Restart, key: "log_dir", flag: "--log-dir",
    }
    Adapter {
        field: adapter, ty: AdapterChoice, default: AdapterChoice::Monitor,
        class: Restart, key: "adapter", flag: "--adapter",
    }
}

/// The settings after the merge, and where each value came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub settings: Settings,
    pub sources: Sources,
}

/// Two chords that can fire on the same key press.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{first} and {second} can both fire on the same key press. Change {first_hint}, or {second_hint}."
)]
pub struct ChordOverlap {
    first: String,
    second: String,
    first_hint: String,
    second_hint: String,
}

/// Why the chords cannot be checked or cannot all be on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    #[error(transparent)]
    Overlap(#[from] ChordOverlap),
    /// A local chord whose key the overlap check does not know. Refused,
    /// because passing it unchecked could hide an overlap.
    #[error(
        "the local chord {chord} ends in a key the chord check does not know; change local_chords"
    )]
    UnknownTrigger { chord: String },
}

/// Flag, then file, then default, per setting; then the checks that need
/// more than one setting.
pub fn resolve(flags: &Layer, file: &Layer) -> Result<Resolved, ChordError> {
    let (settings, sources) = merge(flags, file);
    validate(&settings, &sources)?;
    Ok(Resolved { settings, sources })
}

/// One chord that is on, for the overlap check.
struct Claim {
    key: Key,
    text: String,
    triggers: Vec<Evdev>,
    modifiers: ModifierSet,
    /// A local chord needs its modifiers held exactly; the others fire
    /// whatever else is held.
    local: bool,
}

impl Claim {
    fn open(key: Key, chord: &Chord) -> Self {
        Self {
            key,
            text: chord.to_string(),
            triggers: chord.trigger_keys().to_vec(),
            modifiers: chord.modifiers(),
            local: false,
        }
    }

    /// True when one key press can complete both.
    fn overlaps(&self, other: &Self) -> bool {
        if !self.triggers.iter().any(|k| other.triggers.contains(k)) {
            return false;
        }
        match (self.local, other.local) {
            (false, false) => true,
            (true, true) => false,
            (true, false) => other.modifiers.is_subset_of(self.modifiers),
            (false, true) => self.modifiers.is_subset_of(other.modifiers),
        }
    }
}

fn claims(settings: &Settings) -> Result<Vec<Claim>, ChordError> {
    let mut claims = vec![Claim::open(Key::ReleaseChord, &settings.release_chord)];
    if settings.super_l != SuperL::Off {
        claims.push(Claim::open(Key::SuperLChord, &settings.super_l_chord));
    }
    for (key, binding) in [
        (Key::SwitchNext, &settings.switch_next),
        (Key::SwitchPrevious, &settings.switch_previous),
    ] {
        if let Binding::On(chord) = binding {
            claims.push(Claim::open(key, chord));
        }
    }
    if let Binding::On(modifiers) = settings.switch_jump {
        let triggers = (1..=9)
            .filter_map(|digit: u8| digit.to_string().parse::<Chord>().ok())
            .flat_map(|chord| chord.trigger_keys().to_vec())
            .collect();
        claims.push(Claim {
            key: Key::SwitchJump,
            text: format!("{modifiers}+1 to 9"),
            triggers,
            modifiers,
            local: false,
        });
    }
    for chord in &settings.local_chords.0 {
        let Ok(trigger) = chord.trigger_name().parse::<Chord>() else {
            return Err(ChordError::UnknownTrigger {
                chord: chord.to_string(),
            });
        };
        claims.push(Claim {
            key: Key::LocalChords,
            text: chord.to_string(),
            triggers: trigger.trigger_keys().to_vec(),
            modifiers: chord.modifier_set(),
            local: true,
        });
    }
    Ok(claims)
}

fn source_text(key: Key, source: Source) -> String {
    match (source, key.flag()) {
        (Source::Flag, Some(flag)) => format!("the flag {flag}"),
        (Source::Flag, None) => "a flag".to_owned(),
        (Source::File, _) => "config.toml".to_owned(),
        (Source::Default, _) => "the default".to_owned(),
    }
}

fn hint(key: Key) -> String {
    match key.flag() {
        Some(flag) => format!("{flag} or {} in config.toml", key.name()),
        None => format!("{} in config.toml", key.name()),
    }
}

/// Fails when two chords that are on can fire on the same key press.
pub fn validate(settings: &Settings, sources: &Sources) -> Result<(), ChordError> {
    let claims = claims(settings)?;
    for (index, first) in claims.iter().enumerate() {
        for second in &claims[index + 1..] {
            if first.overlaps(second) {
                let describe = |claim: &Claim| {
                    format!(
                        "{} ({}, from {})",
                        claim.key.name(),
                        claim.text,
                        source_text(claim.key, sources.of(claim.key))
                    )
                };
                return Err(ChordOverlap {
                    first: describe(first),
                    second: describe(second),
                    first_hint: hint(first.key),
                    second_hint: hint(second.key),
                }
                .into());
            }
        }
    }
    Ok(())
}

impl Settings {
    /// The folder for the log and ssh's stderr: `log_dir`, else
    /// `<local_app_data>\gliff-windows\logs`, else `<temp>/gliff-windows/logs`.
    pub fn log_dir_or(&self, local_app_data: Option<PathBuf>, temp: PathBuf) -> PathBuf {
        match &self.log_dir {
            Some(dir) => dir.clone(),
            None => local_app_data
                .unwrap_or(temp)
                .join("gliff-windows")
                .join("logs"),
        }
    }

    /// The tab-switch chords the hook watches for.
    pub fn switch_chords(&self) -> crate::chord_watch::SwitchChords {
        let on = |binding: &Binding<Chord>| match binding {
            Binding::On(chord) => Some(chord.clone()),
            Binding::Off => None,
        };
        crate::chord_watch::SwitchChords {
            next: on(&self.switch_next),
            previous: on(&self.switch_previous),
            jump: match self.switch_jump {
                Binding::On(modifiers) => Some(modifiers),
                Binding::Off => None,
            },
        }
    }

    /// What the keyboard path needs; `keymap` is the XKB text for the Windows
    /// layout at start.
    pub fn input_options(&self, keymap: String) -> InputOptions {
        InputOptions {
            keymap,
            release_chord: self.release_chord.clone(),
            stand_in: match self.super_l {
                // The chord is also the fallback when the registry route
                // cannot be used.
                SuperL::Registry | SuperL::Chord => Some(self.super_l_chord.clone()),
                SuperL::Off => None,
            },
            policy: KeyPolicy {
                injected: self.injected_keys,
                media: if self.forward_media_keys {
                    MediaKeys::Forward
                } else {
                    MediaKeys::Local
                },
            },
        }
    }

    pub fn desktop_mode(&self) -> DesktopMode {
        match (&self.output, self.screen) {
            (Some(output), _) => DesktopMode::Mirror {
                output: output.clone(),
            },
            (None, Screen::Own) => DesktopMode::Headless,
            (None, Screen::Mirror) => DesktopMode::mirror_focused(),
        }
    }
}

#[cfg(test)]
mod tests;
