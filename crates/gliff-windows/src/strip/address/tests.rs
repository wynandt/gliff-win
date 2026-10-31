use super::Act;
use super::Address;
use super::Event;
use crate::history::HostTargetError;

fn host(text: &str) -> crate::history::HostTarget {
    text.parse().expect("a host")
}

fn open_bar(tabs: usize) -> Address {
    let mut bar = Address::new();
    bar.input(Event::Open, tabs);
    bar.input(Event::Focus(true), tabs);
    bar
}

fn typed(bar: &mut Address, text: &str, tabs: usize) {
    bar.input(Event::Text(text.to_owned()), tabs);
}

#[test]
fn a_new_bar_is_folded_and_not_typing() {
    let bar = Address::new();
    assert!(!bar.is_open());
    assert!(!bar.typing());
    assert_eq!(bar.text(), "");
}

#[test]
fn the_plus_opens_the_bar_and_asks_for_the_keyboard() {
    let mut bar = Address::new();
    assert_eq!(bar.input(Event::Open, 2), vec![Act::Focus]);
    assert!(bar.is_open());
    // Not typing until the field says it has the keyboard.
    assert!(!bar.typing());
    bar.input(Event::Focus(true), 2);
    assert!(bar.typing());
}

#[test]
fn the_plus_on_an_open_bar_only_asks_for_the_keyboard_again() {
    let mut bar = open_bar(2);
    typed(&mut bar, "me@host", 2);
    assert_eq!(bar.input(Event::Open, 2), vec![Act::Focus]);
    assert_eq!(bar.text(), "me@host");
}

#[test]
fn enter_connects_to_the_typed_machine_and_folds_the_bar() {
    let mut bar = open_bar(1);
    typed(&mut bar, "me@host", 1);
    assert_eq!(
        bar.input(Event::Enter, 1),
        vec![Act::Connect(host("me@host"))]
    );
    assert!(!bar.is_open());
    assert_eq!(bar.text(), "");
    assert!(!bar.typing());
}

#[test]
fn spaces_round_the_text_are_ignored() {
    let mut bar = open_bar(1);
    typed(&mut bar, "  me@host \t", 1);
    assert_eq!(
        bar.input(Event::Enter, 1),
        vec![Act::Connect(host("me@host"))]
    );
}

#[test]
fn the_go_button_does_what_enter_does() {
    let mut a = open_bar(1);
    let mut b = open_bar(1);
    typed(&mut a, "me@host", 1);
    typed(&mut b, "me@host", 1);
    assert_eq!(a.input(Event::Enter, 1), b.input(Event::Go, 1));
}

#[test]
fn enter_on_nothing_typed_does_nothing_and_leaves_the_bar_open() {
    let mut bar = open_bar(1);
    assert_eq!(bar.input(Event::Enter, 1), vec![]);
    assert!(bar.is_open());
    assert_eq!(bar.error(), None);
}

#[test]
fn a_bad_target_is_refused_with_its_reason_and_the_text_stays() {
    let mut bar = open_bar(1);
    typed(&mut bar, "-oProxyCommand=x", 1);
    assert_eq!(bar.input(Event::Enter, 1), vec![]);
    assert!(bar.is_open());
    assert_eq!(bar.text(), "-oProxyCommand=x");
    assert_eq!(bar.error(), Some(&HostTargetError::LeadingDash));
}

#[test]
fn typing_again_clears_the_refusal() {
    let mut bar = open_bar(1);
    typed(&mut bar, "ssh://host", 1);
    bar.input(Event::Enter, 1);
    assert_eq!(bar.error(), Some(&HostTargetError::UrlScheme));
    typed(&mut bar, "host", 1);
    assert_eq!(bar.error(), None);
}

#[test]
fn escape_folds_the_bar_and_forgets_the_text_when_there_are_tabs() {
    let mut bar = open_bar(2);
    typed(&mut bar, "me@bo", 2);
    assert_eq!(bar.input(Event::Escape, 2), vec![]);
    assert!(!bar.is_open());
    assert_eq!(bar.text(), "");
    assert!(!bar.typing());
}

#[test]
fn escape_with_no_tab_only_clears_the_text_since_there_is_nothing_to_go_back_to() {
    let mut bar = open_bar(0);
    typed(&mut bar, "me@bo", 0);
    assert_eq!(
        bar.input(Event::Escape, 0),
        vec![Act::SetText(String::new())]
    );
    assert!(bar.is_open());
    assert_eq!(bar.text(), "");
}

#[test]
fn losing_the_keyboard_folds_an_empty_bar_with_tabs() {
    let mut bar = open_bar(2);
    assert_eq!(bar.input(Event::Focus(false), 2), vec![]);
    assert!(!bar.is_open());
    assert!(!bar.typing());
}

#[test]
fn losing_the_keyboard_keeps_a_bar_with_text_so_a_click_away_loses_nothing() {
    let mut bar = open_bar(2);
    typed(&mut bar, "me@bo", 2);
    bar.input(Event::Focus(false), 2);
    assert!(bar.is_open());
    assert_eq!(bar.text(), "me@bo");
    assert!(!bar.typing());
}

#[test]
fn losing_the_keyboard_never_folds_a_bar_with_no_tab_behind_it() {
    let mut bar = open_bar(0);
    bar.input(Event::Focus(false), 0);
    assert!(bar.is_open());
}

#[test]
fn opening_the_list_does_not_count_as_losing_the_keyboard() {
    let mut bar = open_bar(2);
    bar.input(Event::List(true), 2);
    bar.input(Event::Focus(false), 2);
    assert!(bar.is_open());
    // The list closing without a pick hands the keyboard back.
    assert_eq!(bar.input(Event::List(false), 2), vec![Act::Focus]);
    bar.input(Event::Focus(true), 2);
    assert!(bar.typing());
}

#[test]
fn picking_a_recent_host_fills_the_field_and_takes_the_keyboard_back() {
    let mut bar = open_bar(2);
    typed(&mut bar, "x", 2);
    let acts = bar.input(Event::Pick("me@host".to_owned()), 2);
    assert_eq!(acts, vec![Act::SetText("me@host".to_owned()), Act::Focus]);
    assert_eq!(bar.text(), "me@host");
}

#[test]
fn picking_clears_an_earlier_refusal() {
    let mut bar = open_bar(2);
    typed(&mut bar, "a b", 2);
    bar.input(Event::Enter, 2);
    assert!(bar.error().is_some());
    bar.input(Event::Pick("me@host".to_owned()), 2);
    assert_eq!(bar.error(), None);
}

#[test]
fn a_window_with_no_tab_opens_the_bar_by_itself() {
    let mut bar = Address::new();
    assert_eq!(bar.tabs_changed(0), vec![Act::Focus]);
    assert!(bar.is_open());
    // Said again, nothing more to do.
    assert_eq!(bar.tabs_changed(0), vec![]);
}

#[test]
fn a_window_with_tabs_leaves_the_bar_as_it_is() {
    let mut bar = Address::new();
    assert_eq!(bar.tabs_changed(1), vec![]);
    assert!(!bar.is_open());
    let mut bar = open_bar(0);
    assert_eq!(bar.tabs_changed(1), vec![]);
    assert!(bar.is_open());
}

#[test]
fn the_last_tab_closing_opens_the_bar_so_there_is_somewhere_to_type() {
    let mut bar = Address::new();
    bar.tabs_changed(1);
    assert_eq!(bar.tabs_changed(0), vec![Act::Focus]);
    assert!(bar.is_open());
}

#[test]
fn a_connected_tab_takes_the_bar_back_and_gives_the_keyboard_to_the_picture() {
    let mut bar = open_bar(1);
    assert!(bar.typing());
    let acts = bar.input(Event::Connected, 1);
    assert_eq!(acts, vec![Act::FocusMain]);
    assert!(!bar.is_open());
    assert!(!bar.typing(), "the grab may resume");
}

#[test]
fn a_connected_tab_does_not_take_a_bar_the_user_is_typing_in() {
    let mut bar = open_bar(2);
    typed(&mut bar, "me@other", 2);
    assert_eq!(bar.input(Event::Connected, 2), Vec::new());
    assert!(bar.is_open());
    assert_eq!(bar.text(), "me@other");
}

#[test]
fn a_connected_tab_leaves_a_folded_bar_alone() {
    let mut bar = Address::new();
    assert_eq!(bar.input(Event::Connected, 1), Vec::new());
    assert!(!bar.is_open());
}

#[test]
fn text_set_by_the_program_is_not_typing() {
    // A picked host is put in the field by the program; the field reports
    // the same text back.
    let mut bar = open_bar(1);
    bar.input(Event::Pick("me@host".into()), 1);
    bar.input(Event::Text("me@host".into()), 1);
    assert_eq!(bar.input(Event::Connected, 1), vec![Act::FocusMain]);
}
