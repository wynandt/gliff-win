use super::ChordDecision;
use super::LocalChord;
use super::LocalChordError;
use super::LocalChordList;
use super::LocalChords;
use crate::keymap::Press;

const LWIN: u16 = 0x5b;
const RWIN: u16 = 0x5c;
const LSHIFT: u16 = 0xa0;
const LCTRL: u16 = 0xa2;
const S: u16 = 0x53;
const E: u16 = 0x45;

fn snip() -> LocalChords {
    LocalChords::new(vec![LocalChord::snip()])
}

fn take(modifiers: &[u16]) -> ChordDecision {
    ChordDecision::Take {
        modifiers: modifiers.to_vec(),
        trigger: S,
    }
}

#[test]
fn win_shift_s_is_taken_and_names_the_modifiers_to_replay() {
    let mut chords = snip();
    assert_eq!(
        chords.push(LWIN, Press::Down, false),
        ChordDecision::Forward
    );
    assert_eq!(
        chords.push(LSHIFT, Press::Down, false),
        ChordDecision::Forward
    );
    assert_eq!(chords.push(S, Press::Down, false), take(&[LSHIFT, LWIN]));
}

#[test]
fn either_win_key_and_either_shift_will_do() {
    let mut chords = snip();
    chords.push(RWIN, Press::Down, false);
    chords.push(0xa1, Press::Down, false);
    assert_eq!(chords.push(S, Press::Down, false), take(&[0xa1, RWIN]));
}

#[test]
fn the_repeats_and_the_up_of_the_trigger_are_swallowed_once() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    chords.push(S, Press::Down, false);
    assert_eq!(chords.push(S, Press::Down, false), ChordDecision::Swallow);
    assert_eq!(chords.push(S, Press::Up, false), ChordDecision::Swallow);
    assert_eq!(
        chords.push(S, Press::Up, false),
        ChordDecision::Forward,
        "a second Up is not the chord's"
    );
    // The Up reaches Windows as a local key (the session ended): the take
    // ends all the same, and the next press is the chord again.
    chords.push(S, Press::Down, false);
    chords.push(S, Press::Up, true);
    assert_eq!(chords.push(S, Press::Down, false), take(&[LSHIFT, LWIN]));
}

#[test]
fn other_keys_with_the_same_modifiers_are_forwarded() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    assert_eq!(chords.push(E, Press::Down, false), ChordDecision::Forward);
}

#[test]
fn the_modifiers_must_match_exactly() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    assert_eq!(
        chords.push(S, Press::Down, false),
        ChordDecision::Forward,
        "Win+S alone is the remote's"
    );
    chords.push(S, Press::Up, false);
    chords.push(LSHIFT, Press::Down, false);
    chords.push(LCTRL, Press::Down, false);
    assert_eq!(
        chords.push(S, Press::Down, false),
        ChordDecision::Forward,
        "an extra modifier is not the chord"
    );
}

#[test]
fn a_released_modifier_is_forgotten() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    chords.push(LSHIFT, Press::Up, false);
    assert_eq!(chords.push(S, Press::Down, false), ChordDecision::Forward);
}

#[test]
fn a_key_that_was_down_before_the_grab_is_not_a_trigger() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    assert_eq!(
        chords.push(S, Press::Down, true),
        ChordDecision::Forward,
        "its repeat belongs to Windows"
    );
}

#[test]
fn modifiers_that_were_down_before_the_grab_still_count() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, true);
    chords.push(LSHIFT, Press::Down, false);
    assert_eq!(chords.push(S, Press::Down, false), take(&[LSHIFT, LWIN]));
    chords.push(LWIN, Press::Up, true);
    assert_eq!(chords.held_modifiers(), vec![LSHIFT]);
}

#[test]
fn no_chords_means_every_key_is_forwarded() {
    let mut chords = LocalChords::new(Vec::new());
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    assert_eq!(chords.push(S, Press::Down, false), ChordDecision::Forward);
}

#[test]
fn reset_forgets_held_modifiers_and_a_taken_trigger() {
    let mut chords = snip();
    chords.push(LWIN, Press::Down, false);
    chords.push(LSHIFT, Press::Down, false);
    chords.push(S, Press::Down, false);
    chords.reset();
    assert!(chords.held_modifiers().is_empty());
    assert_eq!(chords.push(S, Press::Up, false), ChordDecision::Forward);
}

#[test]
fn chords_parse_by_name() {
    let chord: LocalChord = " Win + Shift + S ".parse().expect("parses");
    assert_eq!(chord, LocalChord::snip());
    assert_eq!(
        "ctrl+alt+f12".parse::<LocalChord>().expect("parses"),
        LocalChord {
            modifiers: super::CTRL | super::ALT,
            trigger: 0x7b
        }
    );
    assert_eq!(
        "super+tab".parse::<LocalChord>().expect("parses").trigger,
        0x09
    );
}

#[test]
fn bad_chords_are_refused_with_a_reason() {
    assert_eq!("".parse::<LocalChord>(), Err(LocalChordError::Empty));
    assert_eq!("s".parse::<LocalChord>(), Err(LocalChordError::NoModifier));
    assert_eq!(
        "win+shift".parse::<LocalChord>(),
        Err(LocalChordError::ModifierTrigger("shift".to_owned()))
    );
    assert_eq!(
        "win+nope".parse::<LocalChord>(),
        Err(LocalChordError::UnknownKey("nope".to_owned()))
    );
    assert_eq!(
        "f25+s".parse::<LocalChord>(),
        Err(LocalChordError::UnknownKey("f25".to_owned()))
    );
}

#[test]
fn the_list_defaults_to_the_snip_chord_and_none_turns_it_off() {
    assert_eq!(LocalChordList::default().0, vec![LocalChord::snip()]);
    assert_eq!("none".parse::<LocalChordList>(), Ok(LocalChordList(vec![])));
    let two: LocalChordList = "win+shift+s, win+v".parse().expect("parses");
    assert_eq!(two.0.len(), 2);
    assert!("win+shift+s,".parse::<LocalChordList>().is_err());
}

#[test]
fn local_chords_print_in_one_spelling_that_parses_back() {
    for text in ["win+shift+s", "Super+F11", "control+alt+1", "ctrl+tab"] {
        let chord: LocalChord = text.parse().expect("parses");
        assert_eq!(chord.to_string().parse::<LocalChord>(), Ok(chord), "{text}");
    }
    assert_eq!(LocalChord::snip().to_string(), "win+shift+s");
    assert_eq!(LocalChordList::default().to_string(), "win+shift+s");
    assert_eq!(LocalChordList(vec![]).to_string(), "none");
    let two: LocalChordList = "win+shift+s,ctrl+alt+x".parse().expect("parses");
    assert_eq!(two.to_string(), "win+shift+s,ctrl+alt+x");
}

#[test]
fn a_local_chord_reports_its_modifiers_and_trigger() {
    let chord: LocalChord = "ctrl+alt+x".parse().expect("parses");
    assert_eq!(chord.modifier_set().to_string(), "ctrl+alt");
    assert_eq!(chord.trigger_name(), "x");
}
