use super::ServerVersion;

#[test]
fn versions_parse_from_what_the_server_prints() {
    let v = ServerVersion::new;
    for (printed, want) in [
        ("gliff-server 0.4.1\n", Some(v(0, 4, 1))),
        ("gliff-server 1.12.0", Some(v(1, 12, 0))),
        ("\n\ngliff-server 0.5.0-rc1\n", Some(v(0, 5, 0))),
        ("gliff-server 0.5.0+abc123\n", Some(v(0, 5, 0))),
        ("gliff-server 0.4\n", None),
        ("gliff-server 0.4.1.2\n", None),
        ("gliff-server\n", None),
        ("other-server 0.4.1\n", None),
        ("", None),
    ] {
        assert_eq!(ServerVersion::parse(printed), want, "{printed:?}");
    }
}

#[test]
fn versions_order_by_number_not_by_text() {
    assert!(ServerVersion::new(0, 10, 0) > ServerVersion::new(0, 9, 9));
    assert!(ServerVersion::new(1, 0, 0) > ServerVersion::new(0, 99, 99));
}
