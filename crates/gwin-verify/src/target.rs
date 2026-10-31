//! The ssh target the harness is given. It ends up as an argument of `ssh`
//! and of the client, where a leading `-` would be read as an option (ssh
//! takes `-oProxyCommand=...` as a command to run).

use gliff_windows::history::HostTarget;
use gliff_windows::history::HostTargetError;

/// `--host`: the same parser the client and the recent hosts use, so that
/// what the harness runs is a target the client would accept (`user@host`, a
/// bare host or alias, an IPv6 literal with an optional zone). A `host:port`,
/// a bracket or a leading `-` is refused.
pub fn parse_target(text: &str) -> Result<HostTarget, HostTargetError> {
    text.parse()
}

/// The arguments of `ssh` that run `command` on `target`: batch mode, then
/// `--` so that the target is not read as an option, then the command.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn ssh_command_args(target: &str, command: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["-o", "BatchMode=yes", "--", target]
        .iter()
        .map(|a| (*a).to_owned())
        .collect();
    args.extend(command.iter().map(|a| (*a).to_owned()));
    args
}

#[cfg(test)]
mod tests {
    use gliff_windows::history::HostTarget;

    use super::parse_target;
    use super::ssh_command_args;

    #[test]
    fn a_target_that_looks_like_an_option_is_refused() {
        for bad in [
            "-oProxyCommand=calc",
            "-J",
            "--",
            "",
            "user@-oProxyCommand=x",
            "user@host:22",
            "user@%APPDATA%",
            "a%",
            "user@host;id",
        ] {
            assert!(parse_target(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn ordinary_targets_are_kept() {
        for good in ["user@host", "host", "remote.local", "me@fe80::1%eth0"] {
            assert_eq!(
                parse_target(good).as_ref().map(HostTarget::as_str),
                Ok(good)
            );
        }
    }

    #[test]
    fn the_target_follows_the_end_of_options() {
        let args = ssh_command_args("user@host", &["gwin-keys", "log"]);
        let dashes = args.iter().position(|a| a == "--").expect("a `--`");
        let target = args
            .iter()
            .position(|a| a == "user@host")
            .expect("the target");
        assert_eq!(target, dashes + 1);
        assert_eq!(&args[target + 1..], ["gwin-keys", "log"]);
        assert!(
            args[..dashes]
                .iter()
                .all(|a| a == "-o" || a == "BatchMode=yes")
        );
    }
}
