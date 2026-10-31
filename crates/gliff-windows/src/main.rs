//! `gliff-windows <user@host>`.
// A GUI program on Windows: no console window.
#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;
use gliff_windows::cli::Args;
use gliff_windows::cli::Helper;
#[cfg(not(windows))]
use gliff_windows::startup::Startup;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    let raw: Vec<String> = std::env::args().collect();
    let helper = Helper::from_args(&raw);
    if helper.is_some() {
        // The helpers have no console and no target to take a log
        // directory from: use the default one.
        let dir = std::env::var_os("LOCALAPPDATA")
            .map_or_else(std::env::temp_dir, Into::into)
            .join("gliff-windows")
            .join("logs");
        let _ = gliff_windows::logging::init(&dir);
    }
    match helper {
        None => {
            let args = Args::parse();
            if args.print_settings {
                gliff_windows::startup::print_settings(&args)
            } else {
                gliff_windows::app::run(args)
            }
        }
        Some(Err(message)) => {
            gliff_windows::logging::line(&message);
            std::process::ExitCode::from(2)
        }
        Some(Ok(Helper::RestoreLockPolicy)) => {
            let recovery = gliff_windows::lockreg::restore_by_hand();
            gliff_windows::logging::line(&recovery.to_string());
            std::process::ExitCode::from(recovery.exit_code())
        }
        Some(Ok(Helper::LockWatchdog { pid, start })) => {
            let owner = gliff_windows::lock_policy::Owner { pid, start };
            gliff_windows::logging::line(&gliff_windows::lockreg::watchdog(owner));
            std::process::ExitCode::SUCCESS
        }
    }
}

/// Elsewhere the client only parses its arguments: it needs Win32.
#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    let raw: Vec<String> = std::env::args().collect();
    if let Some(helper) = Helper::from_args(&raw) {
        eprintln!("gliff-windows: {helper:?} is a Windows helper mode");
        return std::process::ExitCode::from(2);
    }
    let args = Args::parse();
    if args.print_settings {
        return gliff_windows::startup::print_settings(&args);
    }
    match Startup::load(&args) {
        Ok(startup) => eprintln!(
            "gliff-windows {} runs on Windows only (targets {:?}, mode {:?})",
            env!("CARGO_PKG_VERSION"),
            args.targets,
            startup.resolved.settings.desktop_mode()
        ),
        Err(e) => eprintln!("gliff-windows: {e}"),
    }
    std::process::ExitCode::from(2)
}
