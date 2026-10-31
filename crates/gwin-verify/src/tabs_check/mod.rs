//! `gwin-verify tabs`: start the client with several targets, switch between
//! them with the tab-switch chords the way a keyboard would, and judge what
//! the client's stats file says. The judge is pure and tested on the build
//! machine; only the Windows harness grabs a keyboard, and it is never run
//! unattended on a person's machine.

#[cfg_attr(not(windows), allow(dead_code))]
pub mod judge;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod run;
#[cfg(test)]
mod tests;
#[cfg(windows)]
pub mod win32;
