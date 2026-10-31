use super::Approx;
use super::SCALES;
use super::build;
use super::cases;
use super::file_name;
use super::problems;
use super::shapes;
use gliff_windows::palette::Palette;

#[test]
fn every_failure_has_a_picture_that_passes_at_every_scale() {
    let pal = Palette::DARK;
    for case in cases() {
        for scale in SCALES {
            let built = build(&case, scale, &Approx, &pal);
            let px = shapes(&built);
            let found = problems(&built, &px, false, &pal);
            assert!(found.is_empty(), "{} at {scale}%: {found:?}", case.name);
        }
    }
}

#[test]
fn the_judge_sees_a_button_that_is_not_where_it_was_drawn() {
    let pal = Palette::DARK;
    let case = &cases()[2];
    let built = build(case, 100, &Approx, &pal);
    let mut px = shapes(&built);
    px.fill(0);
    let found = problems(&built, &px, false, &pal);
    assert!(
        found.iter().any(|p| p.contains("primary button")),
        "{found:?}"
    );
}

#[test]
fn the_judge_sees_missing_words() {
    let pal = Palette::DARK;
    let case = &cases()[2];
    let built = build(case, 100, &Approx, &pal);
    let px = shapes(&built);
    let found = problems(&built, &px, true, &pal);
    assert!(found.iter().any(|p| p.contains("no ink")), "{found:?}");
}

#[test]
fn files_are_named_by_failure_and_scale() {
    let case = &cases()[0];
    assert_eq!(
        file_name(case, 125, false),
        "panel-client-missing-125pct.png"
    );
    assert_eq!(
        file_name(case, 200, true),
        "panel-client-missing-shapes-200pct.png"
    );
}

#[test]
fn there_is_a_case_for_every_failure() {
    assert_eq!(cases().len(), 13);
}
