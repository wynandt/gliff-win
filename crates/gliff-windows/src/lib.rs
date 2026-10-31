//! The gliff Windows client.
//!
//! The pure core (tested on the build machine) and, on Windows only, the Win32 shell
//! (`app`, `window`, `present`, `fullscreen`, `cursor`, `waker`).

pub mod altgr;
#[cfg(windows)]
pub mod app;
pub mod bounded_wait;
pub mod change;
pub mod chord_watch;
pub mod cli;
pub mod clip_core;
pub mod clip_engine;
pub mod clip_files;
pub mod clip_formats;
pub mod clip_hub;
pub mod clip_link;
pub mod clip_log;
#[cfg(windows)]
pub mod clip_win;
pub mod config;
pub mod connect;
#[cfg(windows)]
pub mod cursor;
pub mod cursor_image;
pub mod decode;
pub mod failure_panel;
pub mod foreground_watch;
pub mod frame_slot;
#[cfg(windows)]
pub mod fullscreen;
#[cfg(windows)]
pub mod gpu_decode;
#[cfg(windows)]
pub mod grab;
pub mod grab_state;
pub mod h264;
pub mod history;
#[cfg(windows)]
pub mod hook;
pub mod input;
pub mod key_ledger;
pub mod keymap;
pub mod layout;
pub mod link;
pub mod local_chords;
pub mod lock_policy;
#[cfg(windows)]
pub mod lockreg;
pub mod logging;
pub mod machine_choices;
pub mod modifiers;
pub mod palette;
#[cfg(windows)]
pub mod panel_win;
pub mod pass_log;
pub mod pointer;
#[cfg(windows)]
pub mod present;
pub mod present_log;
pub mod present_math;
pub mod reconnect;
pub mod resize_gate;
pub mod route;
pub mod session;
pub mod session_set;
pub mod settings;
pub mod single_instance;
pub mod ssh;
pub mod stages;
pub mod startup;
pub mod stats_out;
pub mod strip;
#[cfg(windows)]
pub mod strip_edit;
#[cfg(windows)]
pub mod strip_gdi;
#[cfg(windows)]
pub mod strip_win;
pub mod super_l;
pub mod system_paths;
pub mod tabs;
#[cfg(test)]
mod test_dir;
#[cfg(windows)]
pub mod test_hooks;
#[cfg(windows)]
pub mod waker;
#[cfg(windows)]
pub mod window;
#[cfg(windows)]
pub mod window_input;
pub mod xkb;
pub mod yuv;
