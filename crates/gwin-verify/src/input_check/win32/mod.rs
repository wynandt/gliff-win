//! The real machine behind `keys`, `pointer` and the lock step: it runs
//! `gliff-windows.exe`, injects input with `SendInput` the way a keyboard
//! and mouse would, reads the remote machine's `gwin-keys` log over ssh, and reads
//! (never writes) the registry. Windows only; the maintainer runs it on the
//! Windows machine as an approved step.

mod foreground;
mod harness;
pub(crate) mod input;
mod lockstep;
mod overlay;
mod sink;

pub use harness::Launch;
pub use harness::Win32Harness;
