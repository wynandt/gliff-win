use super::report;

#[test]
fn a_failed_dpi_awareness_call_is_logged() {
    let failed = report(Err("awareness refused"));
    let Err(line) = failed else {
        panic!("a failed call is not success, got {failed:?}");
    };
    assert_eq!(
        line, "dpi awareness: SetProcessDpiAwarenessContext failed: awareness refused",
        "a failed call is logged"
    );

    let source = include_str!("../app.rs");
    assert!(
        !source.contains("let _ = unsafe { SetProcessDpiAwarenessContext"),
        "SetProcessDpiAwarenessContext's result is still dropped"
    );
    assert_eq!(
        source.matches("log_dpi(&dpi)").count(),
        2,
        "the refused start and the open log both report the call"
    );
    let call = source
        .find("SetProcessDpiAwarenessContext(")
        .expect("the awareness call");
    let opened = source.find("logging::init").expect("the log opens");
    assert!(
        call < opened,
        "the call stays before the log opens and before any window"
    );
    let reported = source
        .find("log_dpi(&dpi)")
        .expect("a failed call is logged from the client");
    assert!(
        reported > opened,
        "the failure is logged after the log opens"
    );
    assert!(
        source.contains("crate::logging::line(&line)"),
        "the failure line is not written to the client log"
    );
    assert!(
        source.contains("crate::dpi::report"),
        "the client does not report the call through dpi::report"
    );
}

#[test]
fn a_successful_dpi_awareness_call_is_not_logged() {
    let ok = report(Ok::<(), &str>(()));
    assert_eq!(ok, Ok(()));
}
