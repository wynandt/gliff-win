use super::KeyGroup;
use super::keys_plan;
use super::pointer_plan;
use super::select_cases;

#[test]
fn the_default_plan_has_every_group_but_the_lock_step() {
    let plan = keys_plan(&[]).expect("plan");
    assert!(plan.len() >= 40, "all groups: {} cases", plan.len());
    assert!(
        plan.iter().any(|c| c.name.contains("SUPER+CTRL+D")),
        "includes the chord table"
    );
    assert!(
        plan.iter().all(|c| !c.name.contains("SUPER+SHIFT+S")),
        "the snip chord is left to a person"
    );
}

#[test]
fn only_picks_groups_and_the_lock_group_adds_no_cases() {
    let system = keys_plan(&[KeyGroup::System]).expect("plan");
    assert_eq!(system.len(), 8, "the system chords");
    assert!(keys_plan(&[KeyGroup::Lock]).expect("plan").is_empty());
    assert!(
        keys_plan(&[KeyGroup::Manual]).expect("plan").is_empty(),
        "the manual group is prompts, not cases"
    );
}

#[test]
fn the_pointer_plan_is_nine_clicks_the_wheel_and_six_buttons() {
    let plan = pointer_plan((1920, 1200));
    assert_eq!(
        plan.len(),
        9 + 3 + 6,
        "clicks, wheel cases, buttons: four and two thumb ones"
    );
}

#[test]
fn case_picks_by_name_text_in_plan_order_and_a_miss_is_an_error() {
    let plan = || keys_plan(&[KeyGroup::Capture]).expect("plan");
    let all = plan().len();
    assert_eq!(select_cases(plan(), &[]).expect("all").len(), all);

    let hover = select_cases(plan(), &["CAPTURE HOVER".to_owned()]).expect("hover");
    assert!(
        hover.len() >= 3 && hover.len() < all,
        "{} of {all}",
        hover.len()
    );
    assert!(hover.iter().all(|c| c.name.contains("capture hover")));

    let one = select_cases(plan(), &["pointer off, then the probe".to_owned()]).expect("one");
    assert_eq!(one.len(), 1);

    let two = select_cases(
        plan(),
        &[
            "pointer off, then the probe".to_owned(),
            "ungrabbed until leave".to_owned(),
        ],
    )
    .expect("two");
    assert_eq!(two.len(), 2);

    assert!(select_cases(plan(), &["no such case".to_owned()]).is_err());
}
