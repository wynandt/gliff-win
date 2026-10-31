//! The config folder, the settings, the history and the start-up report are
//! the same on every platform: none of them may use the `windows` crate. The
//! Win32 shell (`app.rs` and the modules it calls) reads their results.

use std::fs;
use std::path::Path;

const PURE: [&str; 5] = [
    "config.rs",
    "settings.rs",
    "history.rs",
    "startup.rs",
    "test_dir.rs",
];

fn sources_of(name: &str) -> Vec<(String, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let stem = name.trim_end_matches(".rs");
    let mut files = vec![src.join(name)];
    if let Ok(entries) = fs::read_dir(src.join(stem)) {
        files.extend(entries.map(|e| e.expect("entry").path()));
    }
    files
        .into_iter()
        .map(|path| {
            let text = fs::read_to_string(&path).expect("a source file");
            (path.display().to_string(), text)
        })
        .collect()
}

/// The source with its comments and the text of its strings and characters
/// blanked, so that a `//` inside a string does not hide the rest of its line
/// and a comment cannot hide or fake a use. Line breaks stay, so a position in
/// the result is a position in the source.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push_str("  ");
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push_str("  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[i]));
                    i += 1;
                }
            }
        } else if let Some((start, hashes)) = raw_string_at(&chars, i) {
            let open = start + 2 + hashes;
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let mut j = open;
            while j < chars.len() && chars[j..].get(..close.len()) != Some(&close[..]) {
                j += 1;
            }
            let end = (j + close.len()).min(chars.len());
            out.extend(chars[i..end].iter().map(|&c| blank(c)));
            i = end;
        } else if c == '"' {
            out.push(' ');
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                let step = if chars[i] == '\\' { 2 } else { 1 };
                for &c in &chars[i..(i + step).min(chars.len())] {
                    out.push(blank(c));
                }
                i += step;
            }
            out.push(' ');
            i += 1;
        } else if c == '\'' && is_char_literal(&chars, i) {
            // `'x'` closes at i + 2; `'\n'` and `'\''` close after the escape.
            let end = if chars[i + 1] == '\\' {
                i + 3 + chars[i + 3..].iter().position(|&c| c == '\'').unwrap_or(0)
            } else {
                i + 2
            };
            out.extend(std::iter::repeat_n(' ', end + 1 - i));
            i = end + 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// For a raw string starting at `at` (`r"`, `r#"`, `br"`, `cr#"`): the index of
/// its `r` and the number of `#` in its opening.
fn raw_string_at(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let start = match (chars[at], chars.get(at + 1)) {
        ('r', _) => at,
        ('b' | 'c', Some('r')) => at + 1,
        _ => return None,
    };
    if at.checked_sub(1).is_some_and(|b| ident(chars[b])) {
        return None;
    }
    let hashes = chars[start + 1..].iter().take_while(|&&c| c == '#').count();
    (chars.get(start + 1 + hashes) == Some(&'"')).then_some((start, hashes))
}

/// A `'` starts a character literal, not a lifetime, when a closing `'`
/// follows one character (or one escape).
fn is_char_literal(chars: &[char], at: usize) -> bool {
    match chars.get(at + 1) {
        Some('\\') => chars[at + 2..].iter().take(10).any(|&c| c == '\''),
        Some(_) => chars.get(at + 2) == Some(&'\''),
        None => false,
    }
}

/// The lines in which `code` (see [`code_only`]) uses the `windows` crate or
/// one of its support crates (`windows_core`, `windows_targets`, `windows_sys`
/// and the like): the name as the root of a path, in a `use`, an
/// `extern crate` or an expression, over one line or several. The `windows`
/// module of the standard library (`std::os::windows::io`, also in
/// `use std::os::{windows::io}`) and a method or a variable of that name
/// are not uses.
fn windows_uses(code: &str) -> Vec<usize> {
    let chars: Vec<char> = code.chars().collect();
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !ident(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && ident(chars[i]) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let is_crate = word == "windows" || word.starts_with("windows_");
        if !is_crate || chars[start].is_ascii_digit() {
            continue;
        }
        let before = skip_back(&chars, start);
        let after = skip_forward(&chars, i);
        let followed_by_path = chars[after..].starts_with(&[':', ':']);
        let after_crate_or_use = ["crate", "use"]
            .iter()
            .any(|w| word_before(&chars, before) == *w);
        if (followed_by_path || after_crate_or_use) && !is_member(&chars, before) {
            found.push(chars[..start].iter().filter(|&&c| c == '\n').count() + 1);
        }
    }
    found.dedup();
    found
}

/// The index just after the last non-blank character before `at`.
fn skip_back(chars: &[char], at: usize) -> usize {
    let mut end = at;
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    end
}

fn skip_forward(chars: &[char], at: usize) -> usize {
    let mut i = at;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i
}

/// The identifier that ends at `end` (empty when there is none).
fn word_before(chars: &[char], end: usize) -> String {
    let mut start = end;
    while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
        start -= 1;
    }
    chars[start..end].iter().collect()
}

/// True when the name that follows `before` is a member of something else: of
/// a path (`std::os::`, also through an open `{`) or a value (`items.`).
fn is_member(chars: &[char], before: usize) -> bool {
    if before > 0 && chars[before - 1] == '.' {
        return true;
    }
    let mut end = before;
    // Inside `{ .. }` of a use list, the path before the `{` is the parent.
    let mut depth = 0_i32;
    let mut at = before;
    while at > 0 {
        match chars[at - 1] {
            '}' => depth += 1,
            '{' if depth == 0 => {
                end = skip_back(chars, at - 1);
                break;
            }
            '{' => depth -= 1,
            ';' => break,
            _ => {}
        }
        at -= 1;
    }
    if end < 2 || chars[end - 2..end] != [':', ':'] {
        return false;
    }
    // `use ::windows::..` starts at the root; `std::os::windows` does not.
    let word = word_before(chars, skip_back(chars, end - 2));
    !word.is_empty() && word != "use"
}

#[test]
fn the_pure_modules_do_not_use_the_windows_crate() {
    for name in PURE {
        let files = sources_of(name);
        assert!(!files.is_empty(), "{name} was not found");
        for (path, text) in files {
            let uses = windows_uses(&code_only(&text));
            assert!(
                uses.is_empty(),
                "{path}:{uses:?}: the pure modules do not use the windows crate"
            );
        }
    }
}

#[test]
fn a_use_after_a_slash_slash_in_a_string_is_still_seen() {
    let source = "let url = \"http://x\"; use windows::Win32::Foo;\n";
    assert_eq!(windows_uses(&code_only(source)), [1]);
}

#[test]
fn comments_and_strings_do_not_count_as_uses() {
    let source = concat!(
        "// use windows::A;\n",
        "/* use windows::B; /* nested */ use windows::C; */\n",
        "let s = \"windows::D \\\" windows::E\";\n",
        "let r = r#\"windows::F \" windows::G\"#;\n",
        "let c = '\"'; let l: &'static str = \"x\";\n",
        "let w = items.windows(2); let windows = 3;\n",
        "use std::fmt;\n",
        "use std::os::windows::io::AsRawHandle;\n",
        "use std::os::{windows::io::RawHandle, raw::c_int};\n",
        "let p = std::os::windows::ffi::OsStrExt::encode_wide;\n",
        "let b = br\"windows::H\"; let d = br#\"windows::I \" windows::J\"#;\n",
        "let e = cr\"windows::K\"; let f = cr##\"x\"# windows::L\"##; let g = b\"windows::M\";\n",
        "let windows_count = 3; let n = windows_count + 1;\n",
    );
    assert_eq!(windows_uses(&code_only(source)), Vec::<usize>::new());
}

#[test]
fn real_uses_are_seen_in_every_form() {
    let source = concat!(
        "use windows::Win32::A;\n",
        "use ::windows::Win32::B;\n",
        "extern crate windows as w;\n",
        "use windows_sys::Win32::C;\n",
        "let x = windows :: core::D;\n",
        "let c = '\\''; use windows::E;\n",
        "use windows_core::F;\n",
        "use windows_targets::G;\n",
        "let x = windows_core::H;\n",
    );
    assert_eq!(
        windows_uses(&code_only(source)),
        [1, 2, 3, 4, 5, 6, 7, 8, 9]
    );
}

#[test]
fn a_path_split_over_lines_is_seen() {
    let source = "use windows\n    ::Win32::A;\nuse\n  windows\n::B;\nuse std::os::{\n    windows::io::C,\n    raw::c_int,\n};\nuse {\n  windows::D };\n";
    assert_eq!(windows_uses(&code_only(source)), [1, 4, 11]);
}

#[test]
fn a_use_after_a_raw_byte_string_is_still_seen() {
    let source = "let a = br\"x\"; use windows::A;\nlet b = cr#\"y\"#; use windows_core::B;\n";
    assert_eq!(windows_uses(&code_only(source)), [1, 2]);
}
