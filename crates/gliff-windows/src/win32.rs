//! Two Win32 numbers the client and the checks share.
//!
//! [`CREATE_NO_WINDOW`] is the process-creation flag for a child with no
//! console window. [`WM_MOUSELEAVE`] is the window message sent when the
//! pointer leaves a window that asked to be told.

/// Process-creation flag: the child has no console window (`0x08000000`).
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Window message: the pointer left the window (`0x02A3`).
pub const WM_MOUSELEAVE: u32 = 0x02a3;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;

    use super::CREATE_NO_WINDOW;
    use super::WM_MOUSELEAVE;

    #[test]
    fn one_shared_value_for_each_win32_constant() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
        assert_eq!(WM_MOUSELEAVE, 0x02a3);
        let root = repo_root();
        let mut copies = Vec::new();
        for rel in ["crates/gliff-windows/src", "crates/gwin-verify/src"] {
            collect(&root.join(rel), &root, &mut copies);
        }
        assert!(
            copies.is_empty(),
            "hand-copied Win32 literals remain: {copies:?}"
        );
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root from the gliff-windows manifest")
    }

    fn collect(dir: &Path, root: &Path, copies: &mut Vec<String>) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => panic!("read {}: {err}", dir.display()),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => panic!("read {}: {err}", dir.display()),
            };
            let path = entry.path();
            if path.is_dir() {
                collect(&path, root, copies);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            if path.file_name().and_then(|name| name.to_str()) == Some("win32.rs") {
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) => panic!("read {}: {err}", path.display()),
            };
            let lower = text.to_ascii_lowercase();
            if has_literal(&lower) {
                let shown = path.strip_prefix(root).unwrap_or(&path);
                copies.push(shown.display().to_string());
            }
        }
    }

    fn has_literal(lower: &str) -> bool {
        lower.contains("0x0800_0000") || lower.contains("0x08000000") || has_mouse_leave(lower)
    }

    fn has_mouse_leave(lower: &str) -> bool {
        let bytes = lower.as_bytes();
        let needle = b"0x02a3";
        let mut start = 0;
        while let Some(at) = lower[start..].find("0x02a3") {
            let next = start + at + needle.len();
            let boundary = next >= bytes.len() || !bytes[next].is_ascii_hexdigit();
            if boundary {
                return true;
            }
            start = next;
        }
        false
    }
}
