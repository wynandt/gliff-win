//! `gwin-verify present`: drive the real client's window
//! and judge its stats file.

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod judge;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod run;
#[cfg(windows)]
pub mod win32;
