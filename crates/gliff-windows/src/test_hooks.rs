//! The hidden test flags, `--test-panic` and `--hook-stall-test-ms`, which
//! the verification harness uses against the real client. They exist only in
//! a build with the `test-hooks` feature: a released client has no such flag
//! and its hook callback holds no test code. Without the feature
//! [`TestHooks`] is an empty type whose [`TestHooks::on_key`] does nothing.

#[cfg(feature = "test-hooks")]
mod on {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use crate::cli::Args;
    use crate::keymap::Press;

    /// The only key the stall test applies to.
    const VK_F24: u16 = 0x87;
    /// The key that makes the hook panic under `--test-panic`.
    const VK_F23: u16 = 0x86;

    /// A test-only way to prove the hook survives a slow callback: the next
    /// `times` Downs of F24 make the callback wait `delay`.
    #[derive(Debug)]
    struct Stall {
        delay: Duration,
        times: AtomicU32,
    }

    /// What the flags asked for; nothing unless they were given.
    #[derive(Clone, Debug, Default)]
    pub struct TestHooks {
        stall: Option<Arc<Stall>>,
        /// A test-only way to prove the lock setting is put back after a
        /// panic: an injected F23 Down makes the hook thread panic.
        panic: bool,
    }

    impl TestHooks {
        pub fn from_args(args: &Args) -> Self {
            Self {
                panic: args.test_panic,
                stall: args.hook_stall_test_ms.map(|ms| {
                    Arc::new(Stall {
                        delay: Duration::from_millis(ms),
                        times: AtomicU32::new(u32::MAX),
                    })
                }),
            }
        }

        /// Called by the hook callback for every key it is not passing on as
        /// its own.
        pub fn on_key(&self, vk: u16, press: Press, injected: bool) {
            if self.panic && vk == VK_F23 && press == Press::Down && injected {
                // A panic cannot unwind out of the `extern "system"` callback
                // (Rust 1.81 and later): after the panic hook has put the lock
                // setting back (`lockreg::restore_now`) the process aborts.
                // That is the path this test exercises, and the watchdog
                // helper covers it if the hook itself did not run.
                panic!("--test-panic: the hook thread was asked to panic (injected F23)");
            }
            self.stall(vk, press);
        }

        fn stall(&self, vk: u16, press: Press) {
            let Some(stall) = &self.stall else {
                return;
            };
            if vk != VK_F24 || press != Press::Down {
                return;
            }
            // Take one of the remaining stalls, if any are left.
            let mut left = stall.times.load(Ordering::Acquire);
            loop {
                if left == 0 {
                    return;
                }
                match stall.times.compare_exchange(
                    left,
                    left - 1,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(now) => left = now,
                }
            }
            std::thread::sleep(stall.delay);
        }
    }
}

#[cfg(feature = "test-hooks")]
pub use on::TestHooks;

/// No flag exists in this build, so there is nothing to ask for.
#[cfg(not(feature = "test-hooks"))]
use crate::cli::Args;
#[cfg(not(feature = "test-hooks"))]
use crate::keymap::Press;

#[cfg(not(feature = "test-hooks"))]
#[derive(Clone, Debug, Default)]
pub struct TestHooks;

#[cfg(not(feature = "test-hooks"))]
impl TestHooks {
    pub fn from_args(_args: &Args) -> Self {
        Self
    }

    pub fn on_key(&self, _vk: u16, _press: Press, _injected: bool) {}
}
