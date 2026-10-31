use gliff_proto::ClipboardFile;

use super::Debounce;
use super::EchoGuard;
use super::LocalChange;
use super::local_change;
use super::remote_write;
use crate::clip_formats::Render;
use crate::clip_formats::WinFormat;
use crate::clip_log::Own;

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn a_file() -> ClipboardFile {
    ClipboardFile {
        path: "a.txt".to_owned(),
        size: 1,
        dir: false,
    }
}

#[test]
fn a_private_or_empty_or_unmapped_clipboard_withdraws() {
    assert_eq!(
        local_change(&[WinFormat::UnicodeText], true),
        LocalChange::Withdraw
    );
    assert_eq!(local_change(&[], false), LocalChange::Withdraw);
    assert_eq!(
        local_change(
            &[WinFormat::Registered("Shell IDList Array".to_owned())],
            false
        ),
        LocalChange::Withdraw
    );
    assert_eq!(
        local_change(&[WinFormat::Hdrop], true),
        LocalChange::Withdraw
    );
}

#[test]
fn text_and_files_are_offered_together() {
    let change = local_change(&[WinFormat::Hdrop, WinFormat::UnicodeText], false);
    let LocalChange::Offer { mimes, files } = change else {
        panic!("expected an offer, got {change:?}");
    };
    assert!(files);
    assert_eq!(mimes[0], "text/plain;charset=utf-8");
    assert_eq!(
        local_change(&[WinFormat::Hdrop], false),
        LocalChange::Offer {
            mimes: vec![],
            files: true
        }
    );
}

#[test]
fn remote_offers_are_private_unless_history_is_allowed() {
    let mimes = strings(&["text/plain"]);
    let write = remote_write(&mimes, &[], false).expect("write");
    assert!(write.private);
    assert!(!write.files);
    assert_eq!(write.plans[0].render, Render::Text);
    assert!(!remote_write(&mimes, &[], true).expect("write").private);
}

#[test]
fn a_secret_stays_private_even_with_history() {
    let mimes = strings(&["text/plain", "x-kde-passwordManagerHint"]);
    assert!(remote_write(&mimes, &[], true).expect("write").private);
}

#[test]
fn a_remote_offer_with_nothing_usable_clears() {
    assert_eq!(remote_write(&[], &[], false), None);
    assert_eq!(remote_write(&strings(&["text/uri-list"]), &[], false), None);
    let with_files = remote_write(&[], &[a_file()], false).expect("files");
    assert!(with_files.files && with_files.plans.is_empty());
}

#[test]
fn remote_files_are_private_unless_history_is_allowed_and_secrets_never_are() {
    let files = [a_file()];
    let plain = remote_write(&[], &files, false).expect("write");
    assert!(plain.files && plain.private);
    let open = remote_write(&[], &files, true).expect("write");
    assert!(open.files && !open.private);
    let secret = strings(&["x-kde-passwordManagerHint"]);
    assert!(remote_write(&secret, &files, true).expect("write").private);
}

#[test]
fn our_own_writes_are_not_echoed_back() {
    let mut guard = EchoGuard::default();
    assert!(!guard.is_echo(10, false, false), "nothing written yet");
    guard.wrote(11);
    assert!(guard.is_echo(11, false, false), "the recorded sequence");
    assert!(guard.is_echo(12, true, false), "our window owns it");
    assert!(guard.is_echo(13, false, true), "our marker is on it");
    assert!(!guard.is_echo(14, false, false), "a later copy by the user");
}

#[test]
fn the_verdict_names_the_sign_of_our_own_write() {
    let mut guard = EchoGuard::default();
    assert_eq!(guard.verdict(10, false, false), Own::No);
    guard.wrote(11);
    assert_eq!(guard.verdict(11, false, false), Own::Sequence);
    assert_eq!(guard.verdict(11, true, true), Own::Owner, "owner first");
    assert_eq!(guard.verdict(12, false, true), Own::Marker);
    assert_eq!(guard.verdict(14, false, false), Own::No);
}

#[test]
fn a_copy_in_one_place_is_one_offer_on_each_side() {
    // The user copies on Windows: not an echo, so an offer goes out.
    let mut guard = EchoGuard::default();
    assert!(!guard.is_echo(1, false, false));
    // The remote side answers with its own offer; we write it and record the
    // sequence. The update that write causes is an echo: no new local offer.
    guard.wrote(2);
    assert!(guard.is_echo(2, true, true));
    // The user copies again: an offer again.
    assert!(!guard.is_echo(3, false, false));
}

#[test]
fn a_burst_of_updates_settles_once() {
    let mut debounce = Debounce::default();
    assert!(debounce.update(), "the first update arms the timer");
    assert!(!debounce.update());
    assert!(!debounce.update());
    assert!(debounce.settle());
    assert!(!debounce.settle(), "nothing pending any more");
    assert!(debounce.update(), "the next burst arms it again");
}

#[test]
fn a_windows_snip_is_offered_as_one_png() {
    // Snipping puts a bitmap in two forms, often a registered PNG too.
    for formats in [
        vec![WinFormat::Dib, WinFormat::DibV5],
        vec![WinFormat::Png, WinFormat::Dib, WinFormat::DibV5],
    ] {
        assert_eq!(
            local_change(&formats, false),
            LocalChange::Offer {
                mimes: strings(&["image/png"]),
                files: false
            },
            "{formats:?}"
        );
    }
}

#[test]
fn a_private_picture_is_not_offered() {
    assert_eq!(
        local_change(&[WinFormat::Png, WinFormat::DibV5], true),
        LocalChange::Withdraw
    );
}

#[test]
fn a_remote_picture_is_advertised_as_png_and_bitmap_and_kept_out_of_history() {
    let mimes = strings(&["image/png"]);
    let write = remote_write(&mimes, &[], false).expect("write");
    assert!(write.private, "history is off unless asked for");
    assert_eq!(
        write
            .plans
            .iter()
            .map(|p| (p.format.clone(), p.render))
            .collect::<Vec<_>>(),
        [
            (WinFormat::Png, Render::Passthrough),
            (WinFormat::DibV5, Render::PngToDib)
        ]
    );
    assert!(!remote_write(&mimes, &[], true).expect("write").private);
}
