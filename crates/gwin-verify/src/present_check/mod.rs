//! `gwin-verify present`: drive the real client's window
//! and judge its stats file.

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "only the Windows build uses this")
)]
pub mod judge;
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "only the Windows build uses this")
)]
pub mod run;
#[cfg(windows)]
pub mod win32;
