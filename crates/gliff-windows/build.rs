//! Restricts where the Windows client looks for DLLs it loads by name.
//!
//! `/DEPENDENTLOADFLAG:0x800` is `LOAD_LIBRARY_SEARCH_SYSTEM32`: a DLL the
//! program or the system loads for it comes from the system folder, not from
//! the program's folder or the current folder.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = |key: &str| std::env::var(key).unwrap_or_default();
    if target("CARGO_CFG_TARGET_OS") == "windows" && target("CARGO_CFG_TARGET_ENV") == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
}
