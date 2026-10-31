use std::cell::RefCell;
use std::path::PathBuf;

use super::MIN_SERVER;
use super::ServerVersion;
use super::check_server;
use crate::cli::DesktopMode;
use crate::connect::SshFailure;
use crate::connect::runner::Cmd;
use crate::connect::runner::Output;
use crate::connect::runner::ToolError;
use crate::connect::runner::Tools;
use crate::ssh::SshSpec;

fn spec() -> SshSpec {
    SshSpec {
        ssh: PathBuf::from("ssh"),
        target: "me@lab-pc".parse().expect("a target"),
        server_path: "gliff-server".into(),
        mode: DesktopMode::Headless,
        log_dir: PathBuf::from("logs"),
        log_tag: None,
    }
}

/// Answers every call the same way and remembers what was asked.
struct Canned {
    answer: RefCell<Option<Result<Output, ToolError>>>,
    asked: RefCell<Vec<Cmd>>,
}

impl Canned {
    fn new(answer: Result<Output, ToolError>) -> Self {
        Self {
            answer: RefCell::new(Some(answer)),
            asked: RefCell::default(),
        }
    }

    fn printing(stdout: &str) -> Self {
        Self::new(Ok(Output {
            stdout: stdout.to_owned(),
            stderr: String::new(),
            exit: Some(0),
        }))
    }

    fn failing(stderr: &str, exit: i32) -> Self {
        Self::new(Ok(Output {
            stdout: String::new(),
            stderr: stderr.to_owned(),
            exit: Some(exit),
        }))
    }
}

impl Tools for Canned {
    fn run(&self, cmd: &Cmd) -> Result<Output, ToolError> {
        self.asked.borrow_mut().push(cmd.clone());
        self.answer.borrow_mut().take().expect("asked once")
    }
}

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

#[test]
fn a_server_at_or_above_the_minimum_passes() {
    for printed in [
        "gliff-server 0.4.1\n",
        "gliff-server 0.4.2\n",
        "gliff-server 2.0.0\n",
    ] {
        let tools = Canned::printing(printed);
        let found = check_server(&tools, &spec()).expect("new enough");
        assert!(found >= MIN_SERVER, "{printed:?}");
    }
}

#[test]
fn an_older_server_says_which_version_it_is() {
    let tools = Canned::printing("gliff-server 0.4.0\n");
    assert_eq!(
        check_server(&tools, &spec()),
        Err(SshFailure::ServerOld {
            found: ServerVersion::new(0, 4, 0)
        })
    );
}

#[test]
fn a_server_that_prints_something_else_is_unknown_not_old() {
    let tools = Canned::printing("hello\n");
    assert!(matches!(
        check_server(&tools, &spec()),
        Err(SshFailure::Other { .. })
    ));
}

#[test]
fn a_shell_that_cannot_find_the_server_is_a_missing_server() {
    let tools = Canned::failing("bash: line 1: gliff-server: command not found\n", 127);
    assert_eq!(
        check_server(&tools, &spec()),
        Err(SshFailure::ServerMissing)
    );
}

#[test]
fn a_failed_sign_in_is_classified_like_a_session() {
    let tools = Canned::failing("me@lab-pc: Permission denied (publickey).\n", 255);
    assert_eq!(check_server(&tools, &spec()), Err(SshFailure::KeyRefused));
}

#[test]
fn a_silent_failure_is_not_taken_for_a_missing_server() {
    let tools = Canned::failing("", 127);
    assert!(matches!(
        check_server(&tools, &spec()),
        Err(SshFailure::Other { .. })
    ));
}

#[test]
fn a_call_that_does_not_finish_is_a_machine_that_did_not_answer() {
    let tools = Canned::new(Err(ToolError::Timeout));
    assert_eq!(check_server(&tools, &spec()), Err(SshFailure::Unreachable));
}

#[test]
fn a_missing_ssh_program_is_a_missing_client() {
    let gone = std::io::Error::from(std::io::ErrorKind::NotFound);
    let tools = Canned::new(Err(ToolError::Spawn(gone)));
    assert_eq!(
        check_server(&tools, &spec()),
        Err(SshFailure::ClientMissing)
    );
}

#[test]
fn the_check_asks_the_server_for_its_version_and_nothing_else() {
    let tools = Canned::printing("gliff-server 0.4.1\n");
    check_server(&tools, &spec()).expect("ok");
    let asked = tools.asked.borrow();
    assert_eq!(asked.len(), 1);
    let args: Vec<String> = asked[0]
        .args()
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args[args.len() - 3..],
        ["me@lab-pc", "gliff-server", "--version"]
    );
    assert_eq!(args[args.len() - 4], "--", "the target follows --");
}

#[test]
fn a_server_path_the_shell_would_misread_is_refused_before_anything_runs() {
    let mut bad = spec();
    bad.server_path = "gliff-server;id".into();
    let tools = Canned::printing("gliff-server 0.4.1\n");
    assert!(matches!(
        check_server(&tools, &bad),
        Err(SshFailure::Other { .. })
    ));
    assert!(tools.asked.borrow().is_empty());
}
