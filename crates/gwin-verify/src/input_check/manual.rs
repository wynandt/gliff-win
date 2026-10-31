//! The chords only a person can press: the ones Windows keeps for itself.
//! Each is a prompt and a yes or no.

use super::cases::Act;
use super::cases::Window;
use super::run::CaseResult;
use super::run::Harness;

/// One thing to press and what must then be true.
pub struct Manual {
    pub name: &'static str,
    pub instruction: &'static str,
}

pub const ITEMS: [Manual; 8] = [
    Manual {
        name: "win-space-goes-remote",
        instruction: "With the client focused, press Win+Space. Answer y if the Windows input switcher did NOT appear on the Windows machine (the chord went to the remote machine). If it appeared, press Esc and answer n.",
    },
    Manual {
        name: "win-shift-s-goes-remote",
        instruction: "With the client focused, press Win+Shift+S. Answer y if the screen-snip bar did NOT appear on the Windows machine (the chord went to the remote machine). If it appeared, press Esc and answer n.",
    },
    Manual {
        name: "ctrl-alt-del",
        instruction: "With the client focused, press Ctrl+Alt+Del. Windows' own screen must appear. Press Esc. Answer y if the client then still works (type A and see it on the remote machine).",
    },
    Manual {
        name: "uac",
        instruction: "Start any program as administrator so a UAC prompt shows. Answer y if the prompt appeared and you could answer it (keys go to Windows, not the remote machine), then press y again after you have dismissed it.",
    },
    Manual {
        name: "alt-tab-goes-remote",
        instruction: "With the client focused, press Alt+Tab. Answer y if Windows did NOT switch windows (the chord went to the remote machine).",
    },
    Manual {
        name: "win-alone-goes-remote",
        instruction: "With the client focused, tap the Windows key. Answer y if Start did NOT open on the Windows machine.",
    },
    Manual {
        name: "release-chord",
        instruction: "With the client focused, press Ctrl+Alt+Home. Answer y if Alt+Tab now switches Windows windows and the title bar shows the client let go; then click the client again and answer y only if keys go remote again.",
    },
    Manual {
        name: "no-stuck-keys",
        instruction: "Hold Shift, click away from the client, release Shift, then click the client and type a. Answer y if the remote machine shows a lowercase a (no stuck Shift on either side).",
    },
];

/// The grab limit of the client the prompts run against, in seconds: long
/// enough for every prompt, since the client lets go of the keyboard when it
/// ends.
const GRAB_TIMEOUT_SECS: &str = "900";

/// Asks each item with a client running and in front, because every prompt
/// says "with the client focused". The client is brought to the front again
/// before each prompt (answering clicks the console, which ends a focus-follow
/// grab), stays up through all of them, and is stopped at the end, which also
/// releases any modifier left down. `ask` gets the harness so it can use the
/// same console; a `n` fails the item with the instruction as the reason.
pub fn run<H: Harness>(h: &mut H, ask: &mut dyn FnMut(&mut H, &str) -> bool) -> Vec<CaseResult> {
    let flags = [
        "--grab-timeout-secs".to_owned(),
        GRAB_TIMEOUT_SECS.to_owned(),
    ];
    if let Err(e) = h.start_client(&flags) {
        return ITEMS
            .iter()
            .map(|item| CaseResult {
                name: format!("manual {}", item.name),
                reasons: vec![format!("the client did not start: {e:#}")],
            })
            .collect();
    }
    let results = ITEMS
        .iter()
        .map(|item| {
            let mut reasons = Vec::new();
            if let Err(e) = h.act(&Act::Focus(Window::Client)) {
                reasons.push(format!("bringing the client to the front failed: {e:#}"));
            } else if ask(h, item.instruction) {
                return CaseResult {
                    name: format!("manual {}", item.name),
                    reasons,
                };
            } else {
                reasons.push(format!("answered no: {}", item.instruction));
            }
            CaseResult {
                name: format!("manual {}", item.name),
                reasons,
            }
        })
        .collect();
    h.stop_client();
    results
}

#[cfg(test)]
mod tests {
    use super::ITEMS;
    use super::run;
    use crate::input_check::cases::Act;
    use crate::input_check::remote_log::Entry;
    use crate::input_check::run::Harness;
    use crate::input_check::run::Seen;

    #[derive(Default)]
    struct Fake {
        events: Vec<String>,
        fail_start: bool,
    }

    impl Harness for Fake {
        fn start_client(&mut self, flags: &[String]) -> anyhow::Result<()> {
            if self.fail_start {
                anyhow::bail!("no window");
            }
            self.events.push(format!("start {}", flags.join(" ")));
            Ok(())
        }

        fn stop_client(&mut self) {
            self.events.push("stop".into());
        }

        fn act(&mut self, act: &Act) -> anyhow::Result<()> {
            if matches!(act, Act::Focus(_)) {
                self.events.push("focus client".into());
            }
            Ok(())
        }

        fn remote_log(&mut self) -> anyhow::Result<Vec<Entry>> {
            Ok(Vec::new())
        }

        fn observe(&mut self) -> Seen {
            Seen::default()
        }

        fn remote_size(&mut self) -> Option<(u32, u32)> {
            None
        }
    }

    #[test]
    fn the_client_runs_in_front_for_every_prompt_and_stops_after_the_last() {
        let mut h = Fake::default();
        let results = run(&mut h, &mut |h, _| {
            h.events.push("ask".into());
            true
        });
        assert!(results.iter().all(|r| r.passed()), "all yes passes");
        assert_eq!(results.len(), ITEMS.len());
        assert_eq!(h.events[0], "start --grab-timeout-secs 900");
        assert_eq!(h.events.last().map(String::as_str), Some("stop"));
        let middle = &h.events[1..h.events.len() - 1];
        assert_eq!(middle.len(), 2 * ITEMS.len());
        for pair in middle.chunks(2) {
            assert_eq!(pair, ["focus client", "ask"], "in front before each prompt");
        }
        assert_eq!(
            h.events.iter().filter(|e| e.starts_with("start")).count(),
            1,
            "one client for all the prompts"
        );
    }

    #[test]
    fn a_no_fails_its_item_and_the_instruction_is_the_reason() {
        let mut h = Fake::default();
        let mut asked = 0;
        let results = run(&mut h, &mut |_, _| {
            asked += 1;
            asked != 2
        });
        assert_eq!(asked, ITEMS.len(), "each item asked once");
        let failed: Vec<_> = results.iter().filter(|r| !r.passed()).collect();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].reasons[0].contains(ITEMS[1].instruction));
    }

    #[test]
    fn a_client_that_does_not_start_fails_every_item_and_asks_nothing() {
        let mut h = Fake {
            fail_start: true,
            ..Fake::default()
        };
        let results = run(&mut h, &mut |_, _| panic!("asked without a client"));
        assert_eq!(results.len(), ITEMS.len());
        assert!(results.iter().all(|r| !r.passed()));
    }
}
