//! Restricts where the Windows client looks for DLLs it loads by name, and
//! embeds the application icon in the executable.
//!
//! `/DEPENDENTLOADFLAG:0x800` is `LOAD_LIBRARY_SEARCH_SYSTEM32`: a DLL the
//! program or the system loads for it comes from the system folder, not from
//! the program's folder or the current folder.
//!
//! The icon is assets/gliff.ico, compiled as resource ID 1, which
//! `app_icon` loads at run time.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = |key: &str| env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    if target("CARGO_CFG_TARGET_OS") == "windows" {
        embed_icon();
    }
}

/// Copy the ICO next to a generated .rc and compile it with llvm-rc (cross)
/// or rc.exe (native MSVC). llvm-rc resolves the ICO relative to the .rc,
/// so both files live in OUT_DIR under a short name.
fn embed_icon() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let ico = manifest
        .join("..")
        .join("..")
        .join("assets")
        .join("gliff.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if !ico.is_file() {
        panic!("missing application icon at {}", ico.display());
    }

    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico_out = out.join("gliff.ico");
    fs::copy(&ico, &ico_out).unwrap_or_else(|e| panic!("copy {}: {e}", ico.display()));
    let rc = out.join("gliff.rc");
    // Resource ID 1 is the ID `app_icon` loads.
    fs::write(&rc, "1 ICON \"gliff.ico\"\r\n")
        .unwrap_or_else(|e| panic!("write {}: {e}", rc.display()));

    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_optional()
        .unwrap_or_else(|e| panic!("embed {}: {e}", rc.display()));
}
