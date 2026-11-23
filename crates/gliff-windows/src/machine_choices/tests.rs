use clap::Parser;

use super::Choice;
use super::Choices;
use super::Connection;
use super::Locks;
use super::Origin;
use super::Rules;
use super::Running;
use super::Scale;
use crate::cli::Args;
use crate::cli::DesktopMode;
use crate::cli::Screen;
use crate::history::HostTarget;
use crate::settings::Layer;
use crate::settings::Resolved;
use crate::settings::resolve;

fn host(text: &str) -> HostTarget {
    text.parse().expect("a target")
}

fn world(flags: &[&str], file: &str) -> (Resolved, Locks) {
    let mut line = vec!["gliff-windows", "user@host"];
    line.extend_from_slice(flags);
    let args = Args::try_parse_from(line).expect("a command line");
    let table: toml::Table = file.parse().expect("test TOML");
    let (file, notes) = Layer::from_table(&table);
    assert!(notes.is_empty(), "no note expected: {notes:?}");
    let resolved = resolve(&args.layer(), &file).expect("no overlap");
    let locks = Locks::new(&args.layer(), &resolved);
    (resolved, locks)
}

fn connection(flags: &[&str], file: &str, choice: Choice) -> (Connection, Resolved) {
    let (resolved, locks) = world(flags, file);
    let connection = Connection::of(&resolved.settings, &resolved.sources, &locks, choice);
    (connection, resolved)
}

fn own() -> Choice {
    Choice {
        screen: Some(Screen::Own),
        share_clipboard: None,
        sharp_colour: None,
        ..Choice::default()
    }
}

fn mirror() -> Choice {
    Choice {
        screen: Some(Screen::Mirror),
        share_clipboard: None,
        sharp_colour: None,
        ..Choice::default()
    }
}

fn sharing(share: bool) -> Choice {
    Choice {
        screen: None,
        share_clipboard: Some(share),
        sharp_colour: None,
        ..Choice::default()
    }
}

#[test]
fn a_machine_with_no_choice_runs_with_the_defaults() {
    let (c, resolved) = connection(&[], "", Choice::default());
    assert_eq!(c.screen, Screen::Mirror);
    assert_eq!(c.screen_origin, Origin::Default);
    assert!(c.share_clipboard);
    assert_eq!(c.share_origin, Origin::Default);
    assert_eq!((&c.screen_lock, &c.share_lock), (&None, &None));
    assert_eq!(
        c.desktop_mode(&resolved.settings),
        DesktopMode::mirror_focused()
    );
    assert_eq!(
        (c.send_win_l, c.send_win_l_origin, &c.send_win_l_lock),
        (false, Origin::Default, &None)
    );
}

#[test]
fn a_machine_with_no_choice_gets_exactly_what_the_settings_say() {
    for flags in [&[][..], &["--headless"], &["--output", "DP-2"]] {
        for file in [
            "",
            "screen = \"own\"",
            "output = \"DP-1\"",
            "screen = \"own\"\noutput = \"DP-1\"",
        ] {
            let (c, resolved) = connection(flags, file, Choice::default());
            assert_eq!(
                c.desktop_mode(&resolved.settings),
                resolved.settings.desktop_mode(),
                "flags {flags:?}, file {file:?}"
            );
        }
    }
}

#[test]
fn a_choice_beats_the_file() {
    let (c, resolved) = connection(&[], "screen = \"mirror\"", own());
    assert_eq!((c.screen, c.screen_origin), (Screen::Own, Origin::Machine));
    assert_eq!(c.desktop_mode(&resolved.settings), DesktopMode::Headless);

    let (c, resolved) = connection(&[], "screen = \"own\"", mirror());
    assert_eq!(
        (c.screen, c.screen_origin),
        (Screen::Mirror, Origin::Machine)
    );
    assert_eq!(
        c.desktop_mode(&resolved.settings),
        DesktopMode::mirror_focused()
    );

    let (c, _) = connection(&[], "", sharing(false));
    assert_eq!(
        (c.share_clipboard, c.share_origin),
        (false, Origin::Machine)
    );

    let send = |on: bool| Choice {
        send_win_l: Some(on),
        ..Choice::default()
    };
    let (c, _) = connection(&[], "super_l = \"registry\"", Choice::default());
    assert_eq!((c.send_win_l, c.send_win_l_origin), (true, Origin::File));
    let (c, _) = connection(&[], "super_l = \"registry\"", send(false));
    assert_eq!(
        (c.send_win_l, c.send_win_l_origin),
        (false, Origin::Machine)
    );
    let (c, _) = connection(&[], "", send(true));
    assert_eq!((c.send_win_l, c.send_win_l_origin), (true, Origin::Machine));
    assert_eq!(c.send_win_l_lock, None);
}

#[test]
fn a_named_output_in_the_file_does_not_lock_the_choice() {
    let file = "output = \"DP-1\"";
    let (c, resolved) = connection(&[], file, mirror());
    assert_eq!(c.screen_lock, None);
    assert_eq!(
        c.desktop_mode(&resolved.settings),
        DesktopMode::Mirror {
            output: "DP-1".to_owned()
        }
    );
    let (c, resolved) = connection(&[], file, own());
    assert_eq!(c.desktop_mode(&resolved.settings), DesktopMode::Headless);
}

#[test]
fn a_flag_locks_the_choice_and_says_why() {
    let (c, resolved) = connection(&["--headless"], "", mirror());
    assert_eq!((c.screen, c.screen_origin), (Screen::Own, Origin::Flag));
    let why = c.screen_lock.clone().expect("locked");
    assert!(why.contains("--headless"), "{why}");
    assert_eq!(c.desktop_mode(&resolved.settings), DesktopMode::Headless);

    let (c, resolved) = connection(&["--output", "DP-2"], "", own());
    assert_eq!((c.screen, c.screen_origin), (Screen::Mirror, Origin::Flag));
    let why = c.screen_lock.clone().expect("locked");
    assert!(why.contains("--output"), "{why}");
    assert_eq!(
        c.desktop_mode(&resolved.settings),
        DesktopMode::Mirror {
            output: "DP-2".to_owned()
        }
    );

    let (c, _) = connection(&["--no-clipboard"], "", sharing(true));
    assert!(!c.share_clipboard);
    assert_eq!(c.share_origin, Origin::Flag);
    let why = c.share_lock.clone().expect("locked");
    assert!(why.contains("--no-clipboard"), "{why}");

    for (flag, value) in [("registry", true), ("chord", false), ("off", false)] {
        let choice = Choice {
            send_win_l: Some(!value),
            ..Choice::default()
        };
        let (c, _) = connection(&["--super-l", flag], "", choice);
        assert_eq!(
            (c.send_win_l, c.send_win_l_origin),
            (value, Origin::Flag),
            "--super-l {flag}"
        );
        assert_eq!(
            c.send_win_l_lock.as_deref(),
            Some("Locked: this window was started with --super-l, which applies to every machine.")
        );
    }
}

#[test]
fn every_lock_reason_is_one_plain_sentence() {
    let reason = |flags: &[&str], file: &str, pick: fn(&Connection) -> Option<String>| {
        let (c, _) = connection(flags, file, sharing(true));
        pick(&c).expect("locked")
    };
    let screen = |c: &Connection| c.screen_lock.clone();
    let share = |c: &Connection| c.share_lock.clone();
    assert_eq!(
        reason(&["--headless"], "", screen),
        "Locked: this window was started with --headless, which applies to every machine."
    );
    assert_eq!(
        reason(&["--output", "DP-2"], "", screen),
        "Locked: this window was started with --output, which applies to every machine."
    );
    assert_eq!(
        reason(&["--no-clipboard"], "", share),
        "Locked: this window was started with --no-clipboard, which turns the clipboard off for every machine."
    );
    assert_eq!(
        reason(&[], "share_clipboard = false", share),
        "Locked: config.toml has share_clipboard = false, which turns the clipboard off for every machine."
    );
}

#[test]
fn no_lock_reason_uses_the_old_command_line_wording() {
    for (flags, file) in [
        (&["--headless"][..], ""),
        (&["--output", "DP-2"], ""),
        (&["--no-clipboard"], ""),
        (&[], "share_clipboard = false"),
    ] {
        let (c, _) = connection(flags, file, sharing(true));
        for why in [c.screen_lock, c.share_lock].into_iter().flatten() {
            assert!(why.starts_with("Locked: "), "{why}");
            assert!(!why.contains("is on the command line"), "{why}");
        }
    }
}

#[test]
fn the_flags_lock_nothing_they_do_not_decide() {
    let (c, _) = connection(&["--no-clipboard"], "", own());
    assert_eq!(c.screen_lock, None);
    assert_eq!(c.screen, Screen::Own);
    let (c, _) = connection(&["--headless"], "", sharing(false));
    assert_eq!(c.share_lock, None);
    assert!(!c.share_clipboard);
}

#[test]
fn the_file_turning_the_clipboard_off_locks_the_share_choice() {
    let (c, _) = connection(&[], "share_clipboard = false", sharing(true));
    assert!(!c.share_clipboard);
    assert_eq!(c.share_origin, Origin::File);
    let why = c.share_lock.clone().expect("locked");
    assert!(why.contains("share_clipboard"), "{why}");
    assert!(why.contains("config.toml"), "{why}");
}

#[test]
fn a_saved_choice_is_remembered_per_machine_without_regard_to_host_case() {
    let mut choices = Choices::default();
    assert_eq!(choices.of(&host("me@One")), Choice::default());
    choices.set_screen(&host("me@One"), Screen::Own);
    choices.set_share_clipboard(&host("me@one"), false);
    assert_eq!(
        choices.of(&host("me@ONE")),
        Choice {
            screen: Some(Screen::Own),
            share_clipboard: Some(false),
            ..Choice::default()
        }
    );
    assert_eq!(choices.of(&host("other@one")), Choice::default());
    assert_eq!(choices.machines().len(), 1);
}

#[test]
fn choices_are_written_one_table_per_machine_and_read_back() {
    let mut choices = Choices::default();
    assert!(choices.to_table().is_empty());
    choices.set_screen(&host("me@one"), Screen::Own);
    choices.set_share_clipboard(&host("me@one"), false);
    choices.set_screen(&host("me@two"), Screen::Mirror);
    choices.set_send_win_l(&host("me@two"), true);
    let table = choices.to_table();
    assert_eq!(
        table,
        "[\"me@one\"]\nscreen = \"own\"\nshare_clipboard = false\n[\"me@two\"]\nscreen = \"mirror\"\nsend_win_l = true\n"
            .parse::<toml::Table>()
            .expect("TOML")
    );
    let mut notes = Vec::new();
    let back = Choices::from_table(&table, &mut notes);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(back.of(&host("me@one")), choices.of(&host("me@one")));
    assert_eq!(back.of(&host("me@two")), choices.of(&host("me@two")));
}

#[test]
fn what_the_file_has_that_is_not_a_choice_is_kept_and_noted() {
    let text = r#"
"me@three" = 5
["me@one"]
screen = "own"
future_key = 3
share_clipboard = "yes"
["not a target"]
screen = "own"
"#;
    let table: toml::Table = text.parse().expect("TOML");
    let mut notes = Vec::new();
    let choices = Choices::from_table(&table, &mut notes);
    assert_eq!(choices.of(&host("me@one")).screen, Some(Screen::Own));
    assert_eq!(choices.of(&host("me@one")).share_clipboard, None);
    assert_eq!(notes.len(), 3, "{notes:?}");
    assert!(
        notes.iter().any(|n| n.contains("share_clipboard")),
        "{notes:?}"
    );
    assert_eq!(choices.to_table(), table);

    let mut choices = choices;
    choices.set_screen(&host("me@one"), Screen::Mirror);
    let written = choices.to_table();
    assert_eq!(written["me@one"]["screen"].as_str(), Some("mirror"));
    assert_eq!(written["me@one"]["future_key"].as_integer(), Some(3));
    assert_eq!(written["me@one"]["share_clipboard"].as_str(), Some("yes"));
    assert_eq!(written["me@three"].as_integer(), Some(5));
    assert!(written.contains_key("not a target"));
}

#[test]
fn the_rules_read_the_book_at_each_call() {
    let (resolved, locks) = world(&[], "");
    let book = super::ChoiceBook::default();
    let rules = Rules::new(resolved, locks, std::rc::Rc::clone(&book));
    let a = host("me@a");
    assert_eq!(rules.desktop_mode(&a), DesktopMode::mirror_focused());
    assert!(rules.shares_clipboard(&a));

    book.borrow_mut().set_screen(&a, Screen::Own);
    book.borrow_mut().set_share_clipboard(&a, false);
    assert_eq!(rules.desktop_mode(&a), DesktopMode::Headless);
    assert!(!rules.shares_clipboard(&a));
    let b = host("me@b");
    assert_eq!(rules.desktop_mode(&b), DesktopMode::mirror_focused());
    assert!(rules.shares_clipboard(&b));
}

fn started(screen: Screen, share_clipboard: bool) -> Running {
    Running {
        screen,
        scale: Scale::Auto,
        share_clipboard,
        sharp_colour: false,
        gpu_decode: true,
    }
}

#[test]
fn nothing_is_pending_when_the_session_started_with_what_is_chosen() {
    let (c, _) = connection(&[], "", Choice::default());
    assert!(!c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&[], "", own());
    assert!(!c.pending(&started(Screen::Own, true)));
    let send = Choice {
        send_win_l: Some(true),
        ..Choice::default()
    };
    let (c, _) = connection(&[], "", send);
    assert!(!c.pending(&started(Screen::Mirror, true)));
}

#[test]
fn a_changed_screen_choice_is_pending() {
    let (c, _) = connection(&[], "", own());
    assert!(c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&[], "", mirror());
    assert!(c.pending(&started(Screen::Own, true)));
}

#[test]
fn a_changed_clipboard_choice_is_pending() {
    let (c, _) = connection(&[], "", sharing(false));
    assert!(c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&[], "", sharing(true));
    assert!(c.pending(&started(Screen::Mirror, false)));
}

#[test]
fn a_choice_that_matches_the_default_is_not_pending() {
    let (c, _) = connection(&[], "", mirror());
    assert!(!c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&[], "", sharing(true));
    assert!(!c.pending(&started(Screen::Mirror, true)));
}

#[test]
fn a_locked_row_never_counts_as_pending() {
    let (c, _) = connection(&["--headless"], "", mirror());
    assert!(c.screen_lock.is_some());
    assert!(!c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&["--no-clipboard"], "", sharing(true));
    assert!(c.share_lock.is_some());
    assert!(!c.pending(&started(Screen::Mirror, true)));
    let (c, _) = connection(&["--headless"], "", sharing(false));
    assert!(c.pending(&started(Screen::Own, true)));
}

#[test]
fn invalid_scale_values_are_ignored_noted_and_kept() {
    for raw in [
        "1.5", "1.0", "0", "-2", "3", "125", "200", "\"Auto\"", "\"1.25\"", "\"2\"", "true",
    ] {
        let text = format!("[\"me@one\"]\nscale = {raw}\n");
        let table: toml::Table = text.parse().expect("TOML");
        let mut notes = Vec::new();
        let choices = Choices::from_table(&table, &mut notes);
        assert!(
            notes.iter().any(|n| n.contains("scale")),
            "expected a note for invalid scale = {raw}: {notes:?}"
        );
        assert_eq!(choices.to_table(), table, "kept invalid scale = {raw}");
    }
}

#[test]
fn valid_scale_values_parse_round_trip_and_canonicalize_two() {
    let text = r#"
["me@auto"]
scale = "auto"
["me@s125"]
scale = 1.25
["me@s160"]
scale = 1.6
["me@s200i"]
scale = 2
["me@s200f"]
scale = 2.0
"#;
    let table: toml::Table = text.parse().expect("TOML");
    let mut notes = Vec::new();
    let choices = Choices::from_table(&table, &mut notes);
    assert!(notes.is_empty(), "{notes:?}");
    assert_eq!(choices.of(&host("me@auto")).scale, Some(Scale::Auto));
    assert_eq!(choices.of(&host("me@s125")).scale, Some(Scale::Percent125));
    assert_eq!(choices.of(&host("me@s160")).scale, Some(Scale::Percent160));
    assert_eq!(choices.of(&host("me@s200i")).scale, Some(Scale::Percent200));
    assert_eq!(choices.of(&host("me@s200f")).scale, Some(Scale::Percent200));
    assert_eq!(choices.of(&host("me@absent")).scale, None);

    let written = choices.to_table();
    assert_eq!(
        written["me@auto"]["scale"],
        toml::Value::String("auto".to_owned())
    );
    assert_eq!(written["me@s125"]["scale"], toml::Value::Float(1.25));
    assert_eq!(written["me@s160"]["scale"], toml::Value::Float(1.6));
    assert_eq!(written["me@s200i"]["scale"], toml::Value::Integer(2));
    assert_eq!(written["me@s200f"]["scale"], toml::Value::Integer(2));

    let mut back_notes = Vec::new();
    let back = Choices::from_table(&written, &mut back_notes);
    assert!(back_notes.is_empty(), "{back_notes:?}");
    for name in ["me@auto", "me@s125", "me@s160", "me@s200i", "me@s200f"] {
        assert_eq!(back.of(&host(name)), choices.of(&host(name)), "{name}");
    }
}

#[test]
fn setting_scale_overwrites_an_earlier_invalid_value() {
    let table: toml::Table = "[\"me@one\"]\nscale = 1.5\n".parse().expect("TOML");
    let mut notes = Vec::new();
    let mut choices = Choices::from_table(&table, &mut notes);
    assert_eq!(notes.len(), 1);
    assert_eq!(choices.of(&host("me@one")).scale, None);

    choices.set_scale(&host("me@one"), Scale::Percent160);
    let written = choices.to_table();
    assert_eq!(written["me@one"]["scale"], toml::Value::Float(1.6));
}

#[test]
fn scale_defaults_to_auto_applies_to_own_screen_and_is_ignored_in_mirror() {
    let (c, _) = connection(&[], "", Choice::default());
    assert_eq!((c.scale, c.scale_origin), (Scale::Auto, Origin::Default));
    assert_eq!(c.effective_scale(), Scale::Auto);

    let mirror_with_scale = Choice {
        screen: Some(Screen::Mirror),
        scale: Some(Scale::Percent200),
        ..Choice::default()
    };
    let (c, _) = connection(&[], "", mirror_with_scale);
    assert_eq!(
        (c.scale, c.scale_origin),
        (Scale::Percent200, Origin::Machine)
    );
    assert_eq!(c.effective_scale(), Scale::Auto);
    assert_eq!(c.running().scale, Scale::Auto);
    assert!(!c.pending(&started(Screen::Mirror, true)));

    let own_with_scale = Choice {
        screen: Some(Screen::Own),
        scale: Some(Scale::Percent160),
        ..Choice::default()
    };
    let (c, _) = connection(&[], "", own_with_scale);
    assert_eq!(c.effective_scale(), Scale::Percent160);
    assert_eq!(c.running().scale, Scale::Percent160);
    assert!(c.pending(&started(Screen::Own, true)));
    assert!(!c.pending(&c.running()));
}

#[test]
fn scale_factors_match_auto_and_fixed_percentages() {
    assert_eq!(Scale::Auto.factor(96), 1.0);
    assert_eq!(Scale::Auto.factor(144), 1.5);
    assert_eq!(Scale::Auto.factor(192), 2.0);
    for dpi in [96, 120, 144, 192] {
        assert_eq!(Scale::Percent125.factor(dpi), 1.25, "dpi {dpi}");
        assert_eq!(Scale::Percent160.factor(dpi), 1.6, "dpi {dpi}");
        assert_eq!(Scale::Percent200.factor(dpi), 2.0, "dpi {dpi}");
    }
}
