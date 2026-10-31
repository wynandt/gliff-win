use super::EditCommand;
use super::Facts;
use super::message;
use super::rows;

fn facts(has_selection: bool, has_text: bool, can_paste: bool) -> Facts {
    Facts {
        has_selection,
        has_text,
        can_paste,
    }
}

fn labels(facts: Facts) -> Vec<&'static str> {
    rows(facts).iter().map(|r| r.label).collect()
}

#[test]
fn the_menu_is_cut_copy_paste_select_all_and_nothing_else() {
    assert_eq!(
        labels(facts(true, true, true)),
        [
            "Cut\tCtrl+X",
            "Copy\tCtrl+C",
            "Paste\tCtrl+V",
            "Select All\tCtrl+A"
        ]
    );
    let commands: Vec<_> = rows(facts(true, true, true))
        .iter()
        .map(|r| r.command)
        .collect();
    assert_eq!(
        commands,
        [
            EditCommand::Cut,
            EditCommand::Copy,
            EditCommand::Paste,
            EditCommand::SelectAll
        ]
    );
}

fn enabled(facts: Facts) -> Vec<bool> {
    rows(facts).iter().map(|r| r.enabled).collect()
}

#[test]
fn an_empty_field_with_no_text_on_the_clipboard_has_nothing_to_do() {
    assert_eq!(enabled(facts(false, false, false)), [false; 4]);
}

#[test]
fn a_selection_turns_cut_and_copy_on() {
    assert_eq!(enabled(facts(true, true, false)), [true, true, false, true]);
}

#[test]
fn text_without_a_selection_allows_only_select_all() {
    assert_eq!(
        enabled(facts(false, true, false)),
        [false, false, false, true]
    );
}

#[test]
fn text_on_the_clipboard_turns_paste_on_even_in_an_empty_field() {
    assert_eq!(
        enabled(facts(false, false, true)),
        [false, false, true, false]
    );
}

#[test]
fn each_row_is_carried_out_by_the_edit_fields_own_message() {
    // WM_CUT, WM_COPY and WM_PASTE, and EM_SETSEL over the whole text, as
    // Ctrl+A does in the field.
    assert_eq!((message(EditCommand::Cut).msg, 0x0300), (0x0300, 0x0300));
    let copy = message(EditCommand::Copy);
    assert_eq!((copy.msg, copy.wparam, copy.lparam), (0x0301, 0, 0));
    let paste = message(EditCommand::Paste);
    assert_eq!((paste.msg, paste.wparam, paste.lparam), (0x0302, 0, 0));
    let all = message(EditCommand::SelectAll);
    assert_eq!((all.msg, all.wparam, all.lparam), (0x00b1, 0, -1));
}
