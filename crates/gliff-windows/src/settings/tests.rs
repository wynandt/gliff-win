use std::collections::HashSet;
use std::path::PathBuf;

use super::Binding;
use super::ChordError;
use super::GrabTimeout;
use super::Key;
use super::Layer;
use super::ModifierSet;
use super::Settings;
use super::Source;
use super::resolve;
use crate::cli::AdapterChoice;
use crate::cli::DesktopMode;
use crate::input::InjectedKeys;
use crate::input::MediaKeys;
use crate::keymap::Chord;
use crate::local_chords::LocalChord;
use crate::local_chords::LocalChordList;

/// A layer from file text, which must hold nothing the loader refuses.
fn layer(text: &str) -> Layer {
    let table: toml::Table = text.parse().expect("test TOML");
    let (layer, notes) = Layer::from_table(&table);
    assert!(notes.is_empty(), "no note expected: {notes:?}");
    layer
}

fn chord(text: &str) -> Chord {
    text.parse().expect("a chord")
}

fn defaults() -> Settings {
    resolve(&Layer::default(), &Layer::default())
        .expect("the defaults do not overlap")
        .settings
}

/// Every key with a file value, and a different value for a flag; neither is
/// the default.
const FILE_AND_FLAG: &[(&str, &str, &str)] = &[
    ("release_chord", "\"shift+esc\"", "\"ctrl+alt+end\""),
    ("grab_follows", "\"hover\"", "\"focus\""),
    ("grab_timeout_secs", "60", "90"),
    ("injected_keys", "\"local\"", "\"forward\""),
    ("forward_media_keys", "true", "false"),
    ("local_chords", "\"win+v\"", "[\"win+shift+x\"]"),
    ("side_buttons", "\"linux-mouse\"", "\"upstream\""),
    ("super_l_chord", "\"ctrl+alt+k\"", "\"ctrl+alt+j\""),
    ("switch_next", "\"ctrl+alt+n\"", "\"ctrl+alt+m\""),
    ("switch_previous", "\"ctrl+alt+p\"", "\"ctrl+alt+o\""),
    ("switch_jump", "\"ctrl+shift\"", "\"alt+shift\""),
    ("keep_in_tray", "true", "false"),
    ("remember_recent_hosts", "false", "true"),
    ("clipboard_history", "true", "false"),
    ("share_clipboard", "false", "true"),
    ("server_path", "\"/opt/gliff-server\"", "\"other-server\""),
    ("ssh", "\"C:/ssh/ssh.exe\"", "\"D:/ssh.exe\""),
    ("screen", "\"own\"", "\"mirror\""),
    ("output", "\"DP-1\"", "\"HDMI-1\""),
    ("super_l", "\"registry\"", "\"off\""),
    ("log_dir", "\"C:/logs\"", "\"D:/logs\""),
    ("adapter", "\"minimum-power\"", "\"high-performance\""),
];

fn table_of(pick: impl Fn(&(&str, &str, &str)) -> Option<String>) -> String {
    FILE_AND_FLAG
        .iter()
        .filter_map(|row| pick(row).map(|value| format!("{} = {value}\n", row.0)))
        .collect()
}

#[test]
fn the_precedence_table_covers_every_setting() {
    let covered: HashSet<&str> = FILE_AND_FLAG.iter().map(|row| row.0).collect();
    let all: HashSet<&str> = Key::ALL.iter().map(|key| key.name()).collect();
    assert_eq!(covered, all);
}

#[test]
fn for_each_setting_a_flag_beats_the_file_beats_the_default() {
    let from_file = layer(&table_of(|row| Some(row.1.to_owned())));
    let by_default = resolve(&Layer::default(), &Layer::default()).expect("the defaults resolve");
    let default_json = by_default.settings.to_json(&by_default.sources);

    let only_file =
        resolve(&Layer::default(), &from_file).expect("the file's values do not overlap");
    for key in Key::ALL {
        assert_eq!(only_file.sources.of(*key), Source::File, "{}", key.name());
        assert_ne!(
            only_file.settings.to_json(&only_file.sources)[key.name()]["value"],
            default_json[key.name()]["value"],
            "{} differs from its default, or the file is not proven",
            key.name()
        );
    }

    for key in Key::ALL {
        let one_flag = layer(&table_of(|row| {
            (row.0 == key.name()).then(|| row.2.to_owned())
        }));
        let got = resolve(&one_flag, &from_file).expect("one flag among the file's values");
        let json = got.settings.to_json(&got.sources);
        for other in Key::ALL {
            let want = if other == key {
                Source::Flag
            } else {
                Source::File
            };
            assert_eq!(
                got.sources.of(*other),
                want,
                "{} with a flag for {}",
                other.name(),
                key.name()
            );
        }
        let flag_only = resolve(&one_flag, &Layer::default()).expect("one flag");
        assert_eq!(
            json[key.name()]["value"],
            flag_only.settings.to_json(&flag_only.sources)[key.name()]["value"],
            "the flag's value, not the file's, for {}",
            key.name()
        );
    }
}

#[test]
fn unknown_keys_are_ignored_and_missing_keys_are_none() {
    let table: toml::Table = "colour = \"blue\"\nrelease_chord = \"shift+esc\""
        .parse()
        .expect("toml");
    let (layer, notes) = Layer::from_table(&table);
    assert!(notes.is_empty(), "an unknown key is not a note: {notes:?}");
    assert_eq!(layer.release_chord, Some(chord("shift+esc")));
    assert_eq!(layer.grab_follows, None);
    assert_eq!(layer.to_table().len(), 1, "only the known key is kept");
}

#[test]
fn a_bad_value_is_a_note_and_the_good_keys_still_load() {
    let table: toml::Table = [
        "release_chord = \"ctrl+banana\"",
        "grab_follows = \"sideways\"",
        "grab_timeout_secs = 0",
        "forward_media_keys = \"yes\"",
        "local_chords = 5",
        "server_path = \"\"",
        "switch_jump = \"banana\"",
        "adapter = \"minimum-power\"",
    ]
    .join("\n")
    .parse()
    .expect("toml");
    let (layer, notes) = Layer::from_table(&table);
    assert_eq!(notes.len(), 7, "{notes:?}");
    for (key, word) in [
        ("release_chord", "banana"),
        ("grab_follows", "sideways"),
        ("grab_timeout_secs", "1 to 86400"),
        ("forward_media_keys", "true or false"),
        ("local_chords", "list of chords"),
        ("server_path", "is not empty"),
        ("switch_jump", "banana"),
    ] {
        assert!(
            notes.iter().any(|n| n.starts_with(key) && n.contains(word)),
            "a note for {key} that says {word}: {notes:?}"
        );
    }
    assert_eq!(layer.adapter, Some(AdapterChoice::MinimumPower));
    assert_eq!(layer.release_chord, None, "the bad value is not used");
}

#[test]
fn a_saved_layer_loads_back_equal() {
    let from_file = layer(&table_of(|row| Some(row.1.to_owned())));
    let saved = from_file.to_table();
    let (loaded, notes) = Layer::from_table(&saved);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(loaded, from_file);
    let written: HashSet<&str> = saved.keys().map(String::as_str).collect();
    let named: HashSet<&str> = Key::ALL.iter().map(|key| key.name()).collect();
    assert_eq!(
        written, named,
        "every setting is saved under its key, and nothing else is"
    );
    assert_eq!(
        Layer::default().to_table().len(),
        0,
        "an empty layer writes nothing"
    );
}

#[test]
fn the_local_chords_take_text_a_list_or_none() {
    for (text, count) in [
        ("\"win+shift+s,win+v\"", 2),
        ("[\"win+shift+s\", \"win+v\"]", 2),
        ("\"none\"", 0),
        ("[]", 0),
    ] {
        let got = layer(&format!("local_chords = {text}"));
        assert_eq!(got.local_chords.expect("set").0.len(), count, "{text}");
    }
}

#[test]
fn the_grab_timeout_is_one_to_a_day() {
    assert_eq!(GrabTimeout::new(1).map(GrabTimeout::get), Ok(1));
    assert_eq!(GrabTimeout::new(86_400).map(GrabTimeout::get), Ok(86_400));
    for bad in ["0", "86401", "-1", "soon", ""] {
        assert!(bad.parse::<GrabTimeout>().is_err(), "{bad:?} is refused");
    }
    assert_eq!("90".parse::<GrabTimeout>().map(GrabTimeout::get), Ok(90));
}

#[test]
fn a_binding_is_a_value_or_none() {
    assert_eq!("None".parse::<Binding<Chord>>(), Ok(Binding::Off));
    assert_eq!(
        "ctrl+alt+x".parse::<Binding<Chord>>(),
        Ok(Binding::On(chord("ctrl+alt+x")))
    );
    assert_eq!(Binding::<Chord>::Off.to_string(), "none");
    assert!("ctrl+banana".parse::<Binding<Chord>>().is_err());
}

fn resolve_text(file: &str) -> Result<super::Resolved, super::ChordError> {
    resolve(&Layer::default(), &layer(file))
}

#[test]
fn two_open_chords_on_the_same_key_overlap_whatever_their_modifiers() {
    for text in [
        "release_chord = \"ctrl+alt+pagedown\"",
        "release_chord = \"alt+pagedown\"",
        "release_chord = \"ctrl+alt+pageup\"",
        "super_l_chord = \"shift+pagedown\"",
        "switch_previous = \"ctrl+pagedown\"",
        "release_chord = \"ctrl+1\"",
        "release_chord = \"ctrl+alt+shift+9\"",
        "super_l_chord = \"alt+5\"",
        "switch_next = \"ctrl+alt+home\"",
    ] {
        assert!(resolve_text(text).is_err(), "{text} overlaps a default");
    }
}

#[test]
fn a_local_chord_overlaps_when_the_open_chord_is_among_its_modifiers() {
    for text in [
        "local_chords = \"ctrl+alt+1\"",
        "local_chords = \"ctrl+alt+shift+1\"",
        "local_chords = \"win+ctrl+alt+l\"",
        "local_chords = \"ctrl+alt+l\"",
    ] {
        assert!(resolve_text(text).is_err(), "{text} overlaps a default");
    }
    for text in [
        "local_chords = \"win+1\"",
        "local_chords = \"ctrl+1\"",
        "local_chords = \"ctrl+l\"",
        "local_chords = \"win+shift+s,win+v,win+shift+t\"",
        "local_chords = \"win+shift+s\"\nswitch_jump = \"win\"\nswitch_next = \"none\"",
    ] {
        assert!(resolve_text(text).is_ok(), "{text} does not overlap");
    }
}

#[test]
fn local_chords_never_overlap_each_other() {
    assert!(resolve_text("local_chords = \"win+s,win+shift+s,ctrl+win+s\"").is_ok());
}

#[test]
fn a_chord_that_is_off_cannot_overlap() {
    let ok = [
        "release_chord = \"ctrl+alt+pagedown\"\nswitch_next = \"none\"",
        "release_chord = \"ctrl+alt+3\"\nswitch_jump = \"none\"",
        "release_chord = \"ctrl+alt+l\"\nsuper_l = \"off\"",
    ];
    for text in ok {
        assert!(resolve_text(text).is_ok(), "{text}");
    }
    assert!(resolve_text("release_chord = \"ctrl+alt+l\"\nsuper_l = \"chord\"").is_err());
    assert!(resolve_text("release_chord = \"ctrl+alt+l\"\nsuper_l = \"registry\"").is_err());
}

#[test]
fn the_overlap_message_names_both_settings_their_sources_and_how_to_fix_it() {
    let flags = Layer {
        release_chord: Some(chord("ctrl+alt+pagedown")),
        ..Layer::default()
    };
    let err = resolve(&flags, &Layer::default())
        .expect_err("overlaps")
        .to_string();
    for part in [
        "release_chord (ctrl+alt+pagedown, from the flag --release-chord)",
        "switch_next (ctrl+alt+pagedown, from the default)",
        "--release-chord or release_chord in config.toml",
        "switch_next in config.toml",
    ] {
        assert!(err.contains(part), "{part:?} in {err:?}");
    }
    let file = resolve_text("super_l_chord = \"ctrl+alt+home\"")
        .expect_err("overlaps")
        .to_string();
    assert!(
        file.contains("super_l_chord (ctrl+alt+home, from config.toml)"),
        "{file}"
    );
}

#[test]
fn two_flags_that_overlap_are_an_error_that_names_both_flags() {
    let flags = Layer {
        release_chord: Some(chord("ctrl+alt+k")),
        super_l_chord: Some(chord("ctrl+alt+k")),
        ..Layer::default()
    };
    let err = resolve(&flags, &Layer::default())
        .expect_err("overlaps")
        .to_string();
    assert!(err.contains("the flag --release-chord"), "{err}");
    assert!(err.contains("the flag --super-l-chord"), "{err}");
}

#[test]
fn a_flag_can_clear_an_overlap_the_file_made() {
    let file = layer("release_chord = \"ctrl+alt+pagedown\"");
    assert!(resolve(&Layer::default(), &file).is_err());
    let flags = Layer {
        release_chord: Some(Chord::ctrl_alt_home()),
        ..Layer::default()
    };
    assert!(resolve(&flags, &file).is_ok());
}

#[test]
fn the_report_gives_each_setting_its_value_source_and_class() {
    let got = resolve(
        &Layer {
            grab_timeout_secs: Some(Some(GrabTimeout::new(30).expect("in range"))),
            ..Layer::default()
        },
        &layer("grab_follows = \"hover\""),
    )
    .expect("resolves");
    let json = got.settings.to_json(&got.sources);
    assert_eq!(json["grab_timeout_secs"]["value"], 30);
    assert_eq!(json["grab_timeout_secs"]["source"], "flag");
    assert_eq!(json["grab_timeout_secs"]["class"], "live");
    assert_eq!(json["grab_follows"]["value"], "hover");
    assert_eq!(json["grab_follows"]["source"], "file");
    assert_eq!(json["release_chord"]["value"], "ctrl+alt+home");
    assert_eq!(json["release_chord"]["source"], "default");
    assert_eq!(json["screen"]["value"], "mirror");
    assert_eq!(json["screen"]["source"], "default");
    assert_eq!(json["screen"]["class"], "reconnect");
    assert_eq!(json["output"]["value"], serde_json::Value::Null);
    assert_eq!(json["output"]["class"], "reconnect");
    assert_eq!(json["adapter"]["class"], "restart");
    assert_eq!(json["switch_jump"]["value"], "ctrl+alt");
    assert_eq!(json["local_chords"]["value"], "win+shift+s");
    assert_eq!(json.as_object().expect("an object").len(), Key::ALL.len());
}

#[test]
fn input_options_follow_the_settings() {
    let s = resolve_text("injected_keys = \"local\"\nforward_media_keys = true\nsuper_l = \"off\"")
        .expect("resolves")
        .settings;
    let options = s.input_options("xkb text".to_owned());
    assert_eq!(options.keymap, "xkb text");
    assert_eq!(options.policy.injected, InjectedKeys::Local);
    assert_eq!(options.policy.media, MediaKeys::Forward);
    assert_eq!(options.stand_in, None, "super_l off has no chord");
    assert_eq!(options.release_chord, Chord::ctrl_alt_home());
    for mode in ["chord", "registry"] {
        let s = resolve_text(&format!("super_l = \"{mode}\""))
            .expect("resolves")
            .settings;
        assert_eq!(
            s.input_options(String::new()).stand_in,
            Some(Chord::ctrl_alt_l()),
            "mode {mode}"
        );
    }
}

/// The server's command line for these settings, as the client builds it.
fn server_words(settings: &Settings) -> Vec<String> {
    let spec = crate::ssh::SshSpec {
        ssh: PathBuf::from("ssh.exe"),
        target: "user@host".parse().expect("a target"),
        server_path: "gliff-server".to_owned(),
        mode: settings.desktop_mode(),
        log_dir: PathBuf::from("logs"),
        log_tag: None,
    };
    let args: Vec<String> = crate::ssh::ssh_args(&spec)
        .into_iter()
        .map(|a| a.into_string().expect("ASCII args"))
        .collect();
    let at = args
        .iter()
        .position(|a| a == "gliff-server")
        .expect("the server");
    args[at..].to_vec()
}

fn mode_of(file_text: &str) -> DesktopMode {
    resolve(&Layer::default(), &layer(file_text))
        .expect("resolves")
        .settings
        .desktop_mode()
}

#[test]
fn a_file_with_neither_screen_nor_output_now_mirrors() {
    assert_eq!(
        mode_of("grab_follows = \"hover\""),
        DesktopMode::mirror_focused()
    );
}

#[test]
fn a_file_that_sets_screen_keeps_it() {
    assert_eq!(mode_of("screen = \"own\""), DesktopMode::Headless);
    assert_eq!(
        mode_of("screen = \"mirror\""),
        DesktopMode::mirror_focused()
    );
    let own = resolve(&Layer::default(), &layer("screen = \"own\"")).expect("resolves");
    assert_eq!(
        server_words(&own.settings),
        ["gliff-server", "--stdio", "--headless"]
    );
}

#[test]
fn a_named_output_mirrors_it_and_beats_screen() {
    let named = DesktopMode::Mirror {
        output: "DP-1".to_owned(),
    };
    assert_eq!(mode_of("output = \"DP-1\""), named);
    assert_eq!(mode_of("output = \"DP-1\"\nscreen = \"own\""), named);
    let resolved = resolve(&Layer::default(), &layer("output = \"DP-1\"")).expect("resolves");
    assert_eq!(
        server_words(&resolved.settings),
        ["gliff-server", "--stdio", "--output", "DP-1"]
    );
}

#[test]
fn a_bad_screen_value_is_a_note_and_the_default_stays() {
    let table: toml::Table = "screen = \"both\"".parse().expect("toml");
    let (layer, notes) = Layer::from_table(&table);
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(layer.screen, None);
}

#[test]
fn the_log_folder_is_the_setting_else_local_app_data_else_temp() {
    let temp = PathBuf::from("/tmp");
    let s = defaults();
    assert_eq!(
        s.log_dir_or(Some(PathBuf::from("/appdata")), temp.clone()),
        PathBuf::from("/appdata/gliff-windows/logs")
    );
    assert_eq!(
        s.log_dir_or(None, temp.clone()),
        PathBuf::from("/tmp/gliff-windows/logs")
    );
    let set = resolve_text("log_dir = \"/logs\"")
        .expect("resolves")
        .settings;
    assert_eq!(set.log_dir_or(None, temp), PathBuf::from("/logs"));
}

#[test]
fn a_local_chord_that_ends_in_a_modifier_key_is_refused_when_read() {
    use crate::local_chords::LocalChordError;

    assert_eq!(
        "ctrl+shift".parse::<LocalChord>(),
        Err(LocalChordError::ModifierTrigger("shift".to_owned()))
    );
    let table: toml::Table = "local_chords = \"ctrl+shift\"".parse().expect("TOML");
    let (layer, notes) = Layer::from_table(&table);
    assert_eq!(layer.local_chords, None, "the bad value is left out");
    assert!(notes[0].contains("local_chords"), "{notes:?}");
    assert!(resolve(&Layer::default(), &layer).is_ok());
}

#[test]
fn a_local_chord_the_check_cannot_place_is_an_error_not_a_pass() {
    let mut settings = defaults();
    let sources = resolve(&Layer::default(), &Layer::default())
        .expect("defaults")
        .sources;
    settings.local_chords = LocalChordList(vec![LocalChord::from_parts(
        ModifierSet::CTRL.with(ModifierSet::SHIFT),
        0x10, // VK_SHIFT: not a key a local chord can end in
    )]);
    let error = super::validate(&settings, &sources).expect_err("unplaceable");
    assert!(
        matches!(error, ChordError::UnknownTrigger { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("local_chords"), "{error}");
}

#[test]
fn every_key_a_local_chord_can_end_in_is_placed_by_the_check() {
    let names: Vec<String> = ('a'..='z')
        .map(String::from)
        .chain(('0'..='9').map(String::from))
        .chain((1..=24).map(|n| format!("f{n}")))
        .chain(["tab", "enter", "esc", "space"].map(String::from))
        .collect();
    for name in names {
        let chord: LocalChord = format!("win+shift+{name}").parse().expect("a local chord");
        let mut settings = defaults();
        settings.local_chords = LocalChordList(vec![chord]);
        let sources = resolve(&Layer::default(), &Layer::default())
            .expect("defaults")
            .sources;
        let result = super::validate(&settings, &sources);
        assert!(
            !matches!(result, Err(ChordError::UnknownTrigger { .. })),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn a_local_chord_is_refused_exactly_where_a_default_claim_can_fire_with_it() {
    let mut triggers: Vec<String> = ('a'..='z')
        .map(String::from)
        .chain(('0'..='9').map(String::from))
        .chain(["tab", "enter", "return", "esc", "escape", "space"].map(String::from))
        .collect();
    triggers.extend((1..=24).flat_map(|n| [format!("f{n}"), format!("f{n:02}")]));
    let modifiers = [
        "ctrl",
        "alt",
        "shift",
        "win",
        "ctrl+alt",
        "ctrl+shift",
        "alt+shift",
        "win+shift",
        "ctrl+alt+shift",
        "ctrl+alt+win",
        "ctrl+win",
        "ctrl+alt+shift+win",
    ];
    // The default claims are ctrl+alt+L (the lock chord) and ctrl+alt+1..9.
    // A chord that holds ctrl and alt, and any other modifier on top, fires
    // together with a claim on the same key.
    let claimed = |mods: &str, trigger: &str| {
        let on_claim = trigger == "l"
            || matches!(trigger, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9");
        on_claim
            && mods
                .split('+')
                .filter(|m| matches!(*m, "ctrl" | "alt"))
                .count()
                == 2
    };
    let mut checked = 0;
    for mods in modifiers {
        for trigger in &triggers {
            let spec = format!("{mods}+{trigger}");
            let Ok(chord) = spec.parse::<LocalChord>() else {
                continue;
            };
            let layer = Layer {
                local_chords: Some(LocalChordList(vec![chord])),
                ..Layer::default()
            };
            let result = resolve(&Layer::default(), &layer);
            checked += 1;
            match (claimed(mods, trigger), result) {
                (false, Ok(_)) => {}
                (true, Err(error)) => {
                    let text = error.to_string();
                    assert!(
                        text.contains("can both fire on the same key press")
                            && text.contains("local_chords"),
                        "{spec}: {text}"
                    );
                    assert!(
                        !matches!(error, ChordError::UnknownTrigger { .. }),
                        "{spec}: {error}"
                    );
                }
                (false, Err(error)) => panic!("{spec} was refused: {error}"),
                (true, Ok(_)) => panic!("{spec} overlaps a default claim and was accepted"),
            }
        }
    }
    assert!(checked > 500, "the sweep covered {checked} chords");
}

#[test]
fn the_switch_chords_follow_the_settings() {
    use crate::chord_watch::SwitchChords;
    let settings = defaults();
    assert_eq!(settings.switch_chords(), SwitchChords::standard());
    let mut settings = defaults();
    settings.switch_next = Binding::Off;
    settings.switch_jump = Binding::Off;
    settings.switch_previous = Binding::On(chord("ctrl+alt+left"));
    let chords = settings.switch_chords();
    assert_eq!(chords.next, None);
    assert_eq!(chords.jump, None);
    assert_eq!(chords.previous, Some(chord("ctrl+alt+left")));
}
