//! File trees for the `files` case, and everything the case needs to judge
//! them without the client's code: generated contents, a tar writer and
//! reader (the remote machine's `tar` moves the bytes), `file://` lists,
//! `CF_HDROP`, and a walk of a directory on disk.
//!
//! A tree is compared by path, size and SHA-256, never by anything the
//! client computed.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::bail;

use super::sha256::Sha256;
use super::sha256::hex;
use super::sha256::sha256;

/// `len` bytes that do not repeat within a chunk: a linear congruential
/// stream from `seed`.
pub fn pattern(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state.to_le_bytes()[3]
        })
        .collect()
}

/// One entry of a tree to be created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Dir(String),
    File { path: String, len: usize, seed: u32 },
}

impl Node {
    pub fn path(&self) -> &str {
        match self {
            Self::Dir(path) | Self::File { path, .. } => path,
        }
    }
}

/// What a tree holds, by path with `/`; `None` for a folder, else the size
/// and the SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    pub path: String,
    pub file: Option<(u64, [u8; 32])>,
}

/// A tree to create on one machine, parents before children.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub nodes: Vec<Node>,
}

impl Spec {
    /// The top-level names, in order, once each: what a file manager copies.
    pub fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = Vec::new();
        for node in &self.nodes {
            let root = node.path().split('/').next().unwrap_or("").to_owned();
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }

    /// What the tree is, from the generated contents.
    pub fn found(&self) -> Vec<Found> {
        let mut found: Vec<Found> = self
            .nodes
            .iter()
            .map(|node| match node {
                Node::Dir(path) => Found {
                    path: path.clone(),
                    file: None,
                },
                Node::File { path, len, seed } => Found {
                    path: path.clone(),
                    file: Some((*len as u64, sha256(&pattern(*len, *seed)))),
                },
            })
            .collect();
        found.sort();
        found
    }

    /// Creates the tree under `dir`.
    pub fn write_to(&self, dir: &Path) -> io::Result<()> {
        for node in &self.nodes {
            let path = dir.join(node.path());
            match node {
                Node::Dir(_) => std::fs::create_dir_all(&path)?,
                Node::File { len, seed, .. } => {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&path, pattern(*len, *seed))?;
                }
            }
        }
        Ok(())
    }

    /// The tree as a ustar archive.
    pub fn tar(&self) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::new();
        for node in &self.nodes {
            match node {
                Node::Dir(path) => out.extend(header(&format!("{path}/"), 0, b'5')?),
                Node::File { path, len, seed } => {
                    out.extend(header(path, *len as u64, b'0')?);
                    out.extend(pattern(*len, *seed));
                    out.resize(out.len().next_multiple_of(512), 0);
                }
            }
        }
        out.resize(out.len() + 1024, 0);
        Ok(out)
    }
}

fn octal(field: &mut [u8], value: u64) {
    let digits = format!("{value:0width$o}", width = field.len() - 1);
    field[..digits.len()].copy_from_slice(digits.as_bytes());
}

/// One ustar header. A name over 100 bytes is split at a `/` into prefix and
/// name, which holds up to 255 bytes.
fn header(name: &str, size: u64, kind: u8) -> anyhow::Result<[u8; 512]> {
    let bytes = name.as_bytes();
    let (prefix, base) = if bytes.len() <= 100 {
        (&bytes[..0], bytes)
    } else {
        let at = bytes
            .iter()
            .enumerate()
            .filter(|(i, b)| **b == b'/' && *i <= 155 && bytes.len() - i - 1 <= 100)
            .map(|(i, _)| i)
            .next()
            .with_context(|| format!("the name is too long for ustar: {name}"))?;
        (&bytes[..at], &bytes[at + 1..])
    };
    let mut block = [0u8; 512];
    block[..base.len()].copy_from_slice(base);
    octal(
        &mut block[100..108],
        if kind == b'5' { 0o755 } else { 0o644 },
    );
    octal(&mut block[108..116], 0);
    octal(&mut block[116..124], 0);
    octal(&mut block[124..136], size);
    octal(&mut block[136..148], 0);
    block[148..156].fill(b' ');
    block[156] = kind;
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    block[345..345 + prefix.len()].copy_from_slice(prefix);
    let sum: u32 = block.iter().map(|b| u32::from(*b)).sum();
    let digits = format!("{sum:06o}\0 ");
    block[148..156].copy_from_slice(digits.as_bytes());
    Ok(block)
}

fn field_text(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    &field[..end]
}

fn field_number(field: &[u8]) -> anyhow::Result<u64> {
    if field[0] & 0x80 != 0 {
        // GNU base-256, for sizes over 8 GiB.
        let mut value = u64::from(field[0] & 0x7f);
        for byte in &field[1..] {
            value = value.checked_mul(256).context("size overflow")? + u64::from(*byte);
        }
        return Ok(value);
    }
    let text = String::from_utf8_lossy(field_text(field)).trim().to_owned();
    if text.is_empty() {
        return Ok(0);
    }
    u64::from_str_radix(&text, 8).with_context(|| format!("bad number {text:?}"))
}

/// The `path` of a pax extended header, when it has one.
fn pax_path(data: &[u8]) -> Option<String> {
    let mut rest = data;
    while !rest.is_empty() {
        let space = rest.iter().position(|b| *b == b' ')?;
        let length: usize = std::str::from_utf8(&rest[..space]).ok()?.parse().ok()?;
        let record = rest.get(..length)?;
        let body = record.get(space + 1..length.checked_sub(1)?)?;
        if let Some(path) = body.strip_prefix(b"path=") {
            return String::from_utf8(path.to_vec()).ok();
        }
        rest = &rest[length..];
    }
    None
}

fn clean(name: &str) -> String {
    name.trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

/// What a tar archive holds, with its files hashed as they are read. Reads
/// ustar, GNU long names and pax `path` records; anything but files and
/// folders is an error, so a symlink cannot hide in a comparison.
pub fn read_tar(archive: &[u8]) -> anyhow::Result<Vec<Found>> {
    let mut found = Vec::new();
    let mut at = 0;
    let mut long_name: Option<String> = None;
    let mut ended = false;
    while at + 512 <= archive.len() {
        let block = &archive[at..at + 512];
        at += 512;
        if block.iter().all(|b| *b == 0) {
            ended = true;
            break;
        }
        let size = field_number(&block[124..136])?;
        let end = at
            .checked_add(usize::try_from(size)?)
            .filter(|end| *end <= archive.len())
            .context("the archive ends inside an entry")?;
        let data = &archive[at..end];
        at = end.next_multiple_of(512);
        let kind = block[156];
        match kind {
            b'L' => {
                long_name = Some(String::from_utf8_lossy(field_text(data)).into_owned());
                continue;
            }
            b'x' => {
                long_name = pax_path(data).or(long_name);
                continue;
            }
            b'g' => continue,
            _ => {}
        }
        let name = match long_name.take() {
            Some(name) => name,
            None => {
                let base = String::from_utf8_lossy(field_text(&block[..100])).into_owned();
                let prefix = String::from_utf8_lossy(field_text(&block[345..500])).into_owned();
                if &block[257..262] == b"ustar" && !prefix.is_empty() {
                    format!("{prefix}/{base}")
                } else {
                    base
                }
            }
        };
        let path = clean(&name);
        match kind {
            b'0' | 0 => found.push(Found {
                path,
                file: Some((size, sha256(data))),
            }),
            b'5' => found.push(Found { path, file: None }),
            other => bail!(
                "`{path}` is a tar entry of type {:?}, not a file or folder",
                other as char
            ),
        }
    }
    if !ended {
        bail!("the archive has no end marker: it was cut short");
    }
    found.sort();
    Ok(found)
}

/// Walks `roots` on disk. Each root is listed by its own name, then what is
/// under it, as `name/child`. A symlink is an error: the gate makes none.
pub fn list_disk(roots: &[PathBuf]) -> anyhow::Result<Vec<Found>> {
    fn walk(path: &Path, rel: String, out: &mut Vec<Found>) -> anyhow::Result<()> {
        let meta = std::fs::symlink_metadata(path)
            .with_context(|| format!("reading {}", path.display()))?;
        if meta.is_dir() {
            out.push(Found {
                path: rel.clone(),
                file: None,
            });
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                walk(&entry.path(), format!("{rel}/{name}"), out)?;
            }
        } else if meta.is_file() {
            let mut file =
                std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
            let mut hash = Sha256::default();
            let mut buffer = vec![0u8; 1 << 16];
            let mut size = 0u64;
            loop {
                let n = io::Read::read(&mut file, &mut buffer)?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
                size += n as u64;
            }
            out.push(Found {
                path: rel,
                file: Some((size, hash.finish())),
            });
        } else {
            bail!("{} is neither a file nor a folder", path.display());
        }
        Ok(())
    }
    let mut out = Vec::new();
    for root in roots {
        let name = root
            .file_name()
            .with_context(|| format!("{} has no name", root.display()))?
            .to_string_lossy()
            .into_owned();
        walk(root, name, &mut out)?;
    }
    out.sort();
    Ok(out)
}

/// A `text/uri-list` body for absolute paths: CRLF lines, every byte but
/// letters, digits and `/-._~` percent-encoded.
pub fn uri_list(paths: &[String]) -> String {
    let mut out = String::new();
    for path in paths {
        out.push_str("file://");
        for byte in path.bytes() {
            if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
                out.push(char::from(byte));
            } else {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// The paths of the `file://` lines of a `text/uri-list`, for the lines with
/// no or a `localhost` authority.
pub fn parse_uri_list(body: &str) -> anyhow::Result<Vec<String>> {
    let mut paths = Vec::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.starts_with('#') {
            continue;
        }
        let rest = line
            .strip_prefix("file://")
            .with_context(|| format!("not a file URI: {line}"))?;
        let slash = rest.find('/').context("a file URI without a path")?;
        if !matches!(&rest[..slash], "" | "localhost") {
            bail!("a file URI for another host: {line}");
        }
        let mut bytes = Vec::new();
        let encoded = &rest.as_bytes()[slash..];
        let mut i = 0;
        while i < encoded.len() {
            if encoded[i] == b'%' {
                let hex = encoded.get(i + 1..i + 3).context("a cut percent escape")?;
                bytes.push(u8::from_str_radix(std::str::from_utf8(hex)?, 16)?);
                i += 3;
            } else {
                bytes.push(encoded[i]);
                i += 1;
            }
        }
        paths.push(String::from_utf8(bytes).context("a path that is not UTF-8")?);
    }
    Ok(paths)
}

/// A `CF_HDROP` block: the `DROPFILES` header, then wide paths each ended by
/// NUL, and one more NUL.
pub fn hdrop(paths: &[String]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(20u32.to_le_bytes()); // pFiles
    out.extend([0u8; 8]); // pt
    out.extend(0u32.to_le_bytes()); // fNC
    out.extend(1u32.to_le_bytes()); // fWide
    for path in paths {
        for unit in path.encode_utf16() {
            out.extend(unit.to_le_bytes());
        }
        out.extend([0, 0]);
    }
    out.extend([0, 0]);
    out
}

/// The paths of a `CF_HDROP` block, `None` when it is not a wide one.
pub fn parse_hdrop(block: &[u8]) -> Option<Vec<String>> {
    let offset = usize::try_from(u32::from_le_bytes(block.get(..4)?.try_into().ok()?)).ok()?;
    let wide = u32::from_le_bytes(block.get(16..20)?.try_into().ok()?);
    if wide == 0 || offset < 20 {
        return None;
    }
    let units: Vec<u16> = block
        .get(offset..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u16::from_le_bytes(*b))
        .collect();
    let mut paths = Vec::new();
    for path in units.split(|u| *u == 0) {
        if path.is_empty() {
            break;
        }
        paths.push(String::from_utf16_lossy(path));
    }
    Some(paths)
}

/// Windows would open this name: no reserved character, no trailing dot or
/// space, and not a device name (with or without an extension).
pub fn windows_name_ok(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with('.')
        || name.ends_with(' ')
        || name
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_uppercase();
    let numbered = |prefix: &str| {
        stem.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit() && n != "0")
    };
    !(["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || numbered("COM") || numbered("LPT"))
}

fn short(digest: &[u8; 32]) -> String {
    hex(&digest[..6])
}

/// The first thing that tells `got` from `expected`, or `None` when they
/// are the same tree.
///
/// `strict` wants the same paths. Otherwise a name may have been changed to
/// one Windows can open: the trees need the same folders and the same
/// files by size and SHA-256, every name must be openable, and no two may
/// differ only in case.
pub fn difference(expected: &[Found], got: &[Found], strict: bool) -> Option<String> {
    if strict {
        let want: BTreeMap<&str, &Option<(u64, [u8; 32])>> = expected
            .iter()
            .map(|f| (f.path.as_str(), &f.file))
            .collect();
        let have: BTreeMap<&str, &Option<(u64, [u8; 32])>> =
            got.iter().map(|f| (f.path.as_str(), &f.file)).collect();
        if let Some(path) = want.keys().find(|p| !have.contains_key(*p)) {
            return Some(format!("`{path}` is missing"));
        }
        if let Some(path) = have.keys().find(|p| !want.contains_key(*p)) {
            return Some(format!("`{path}` is extra"));
        }
        for (path, file) in &want {
            match (file, have[path]) {
                (None, None) => {}
                (None, Some(_)) => return Some(format!("`{path}` is a file, not a folder")),
                (Some(_), None) => return Some(format!("`{path}` is a folder, not a file")),
                (Some((size, a)), Some((got_size, b))) => {
                    if size != got_size {
                        return Some(format!("`{path}` has {got_size} bytes, not {size}"));
                    }
                    if a != b {
                        return Some(format!(
                            "`{path}` differs in content: sha256 {} not {}",
                            short(b),
                            short(a)
                        ));
                    }
                }
            }
        }
        return None;
    }
    let folders = |tree: &[Found]| tree.iter().filter(|f| f.file.is_none()).count();
    if folders(expected) != folders(got) {
        return Some(format!(
            "{} folders, not {}",
            folders(got),
            folders(expected)
        ));
    }
    let mut left: Vec<(u64, [u8; 32])> = got.iter().filter_map(|f| f.file).collect();
    for want in expected {
        let Some(file) = want.file else { continue };
        match left.iter().position(|have| *have == file) {
            Some(i) => {
                left.swap_remove(i);
            }
            None => {
                return Some(format!(
                    "no file with the {} bytes and sha256 {} of `{}`",
                    file.0,
                    short(&file.1),
                    want.path
                ));
            }
        }
    }
    if !left.is_empty() {
        return Some(format!("{} files too many", left.len()));
    }
    let mut seen = std::collections::HashSet::new();
    for f in got {
        if let Some(part) = f.path.split('/').find(|part| !windows_name_ok(part)) {
            return Some(format!(
                "`{}` has a name Windows cannot open: `{part}`",
                f.path
            ));
        }
        if !seen.insert(f.path.to_lowercase()) {
            return Some(format!(
                "`{}` differs from another name only in case",
                f.path
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests;
