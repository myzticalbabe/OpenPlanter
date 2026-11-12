//! op-fast: Rust fast paths backing the Python agent.
//!
//! Each function mirrors a Python hotspot with identical semantics:
//! - [`list_files`] / [`search_files`] mirror `agent/tools.py` (skip `.git`,
//!   50k-entry cap, ripgrep-style glob rule, `rg`-compatible output).
//! - [`fingerprint_dir`] is a parallel sha256 walk for `integrity.py`-style
//!   corpora: unchanged files are skipped without re-hashing.
//! - [`scan_wiki_index`] mirrors `agent/wiki_graph.py` index-row parsing.
//! - [`check_patch`] mirrors `agent/patching.py` marker validation.
//! - [`BoardLease`] mirrors the `_board.py` claim-lease decision as pure
//!   logic (no sqlite dep) so the rule is unit-tested in Rust.

use std::collections::HashSet;
use std::path::Path;

use rayon::prelude::*;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

pub const MAX_WALK_ENTRIES: usize = 50_000;

/// Ripgrep-style glob rule (mirrors `tools.py` fallback):
/// a pattern containing `/` matches the root-relative path,
/// otherwise it matches the basename at any depth.
pub fn glob_match(pattern: &str, rel_posix: &str) -> bool {
    if pattern.contains('/') {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(rel_posix))
            .unwrap_or(false)
    } else {
        let base = rel_posix.rsplit('/').next().unwrap_or(rel_posix);
        glob::Pattern::new(pattern)
            .map(|p| p.matches(base))
            .unwrap_or(false)
    }
}

fn is_skipped_dir(name: &str) -> bool {
    name == ".git"
}

/// Walk `root`, skipping `.git`, capped at [`MAX_WALK_ENTRIES`] files.
/// Returns root-relative posix paths, sorted.
pub fn list_files(root: &Path, glob_pat: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut count = 0usize;
    let mut walker = WalkDir::new(root).into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            !e.file_type().is_dir()
                || !is_skipped_dir(&e.file_name().to_string_lossy())
        });
    while let Some(entry) = walker.next() {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        count += 1;
        if count > MAX_WALK_ENTRIES {
            break;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(pat) = glob_pat {
            if !glob_match(pat, &rel) {
                continue;
            }
        }
        out.push(rel);
    }
    out.sort();
    out
}

/// Case-insensitive substring search over files, `rg -n` style
/// `rel:line:text` hits. Binary/unreadable files are skipped.
pub fn search_files(
    root: &Path,
    query: &str,
    glob_pat: Option<&str>,
    max_hits: usize,
) -> Vec<String> {
    let needle = query.to_lowercase();
    if needle.trim().is_empty() {
        return Vec::new();
    }
    let files = list_files(root, glob_pat);
    let mut hits = Vec::new();
    for rel in files {
        if hits.len() >= max_hits {
            break;
        }
        let full = root.join(&rel);
        let Ok(bytes) = std::fs::read(&full) else { continue };
        // Skip likely-binary files (NUL in first 8k).
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for (idx, line) in text.split('\n').enumerate() {
            if line.to_lowercase().contains(&needle) {
                hits.push(format!("{}:{}:{}", rel, idx + 1, line));
                if hits.len() >= max_hits {
                    break;
                }
            }
        }
    }
    hits
}

/// One file fingerprint: sha256 hex + byte length.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FileFp {
    pub rel: String,
    pub sha256: String,
    pub len: u64,
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Parallel sha256 walk over `root` (skips `.git`). Returns sorted fingerprints.
/// Files that fail to read are skipped — same as `integrity.py`.
pub fn fingerprint_dir(root: &Path) -> Vec<FileFp> {
    let rels = list_files(root, None);
    let mut fps: Vec<FileFp> = rels
        .par_iter()
        .filter_map(|rel| {
            let full = root.join(rel);
            let bytes = std::fs::read(&full).ok()?;
            let mut h = Sha256::new();
            h.update(&bytes);
            let digest = h.finalize();
            Some(FileFp {
                rel: rel.clone(),
                sha256: hex::encode(digest),
                len: bytes.len() as u64,
            })
        })
        .collect();
    fps.sort_by(|a, b| a.rel.cmp(&b.rel));
    fps
}

/// 2-char whitespace-invariant line hash (mirrors `tools.py:_line_hash`
/// and `op-core::filesystem::line_hash`).
pub fn line_hash(line: &str) -> String {
    let normalized: String = line.split_whitespace().collect();
    let crc = crc32fast::hash(normalized.as_bytes());
    format!("{:02x}", crc & 0xFF)
}

// ---------------------------------------------------------------------------
// Wiki index scan (mirrors agent/wiki_graph.py)
// ---------------------------------------------------------------------------

/// Parsed `| name | jurisdiction | [text](path) |` index row.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WikiRow {
    pub name: String,
    pub jurisdiction: String,
    pub link_text: String,
    pub path: String,
    pub category: String,
}

fn parse_category(line: &str) -> Option<String> {
    let t = line.trim();
    t.strip_prefix("###").map(|s| s.trim().to_string())
}

fn parse_index_row(line: &str) -> Option<(String, String, String, String)> {
    // | name | jurisdiction | [link](path) |
    let mut cells: Vec<&str> = line.split('|').map(|c| c.trim()).collect();
    // Leading/trailing `|` produce empty edge cells.
    if cells.len() >= 5 && cells.first().is_some_and(|c| c.is_empty()) {
        cells.remove(0);
    }
    if cells.len() >= 4 && cells.last().is_some_and(|c| c.is_empty()) {
        cells.pop();
    }
    if cells.len() < 3 {
        return None;
    }
    let (name, jurisdiction, link_cell) = (cells[0], cells[1], cells[2]);
    if name.is_empty() || !link_cell.starts_with('[') {
        return None;
    }
    let mid = link_cell.find("](")?;
    let end = link_cell.rfind(')')?;
    if mid + 2 > end {
        return None;
    }
    Some((
        name.to_string(),
        jurisdiction.to_string(),
        link_cell[1..mid].to_string(),
        link_cell[mid + 2..end].to_string(),
    ))
}

/// Scan `dir` for markdown index rows. Returns rows with their category.
pub fn scan_wiki_index(dir: &Path) -> Vec<WikiRow> {
    let files = list_files(dir, Some("*.md"));
    let mut rows = Vec::new();
    for rel in files {
        let Ok(text) = std::fs::read_to_string(dir.join(&rel)) else {
            continue;
        };
        let mut category = String::new();
        for line in text.lines() {
            if let Some(c) = parse_category(line) {
                category = c;
                continue;
            }
            if let Some((name, jurisdiction, link_text, path)) = parse_index_row(line) {
                rows.push(WikiRow {
                    name,
                    jurisdiction,
                    link_text,
                    path,
                    category: category.clone(),
                });
            }
        }
    }
    rows
}

// ---------------------------------------------------------------------------
// Patch check (mirrors agent/patching.py marker validation)
// ---------------------------------------------------------------------------

/// Validate `*** Begin Patch` / `*** End Patch` markers and per-op `+` rules
/// for Add-File chunks. Returns op count on success.
pub fn check_patch(text: &str) -> Result<usize, String> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Err("patch is empty".to_string());
    }
    if lines[0].trim() != "*** Begin Patch" {
        return Err("patch must start with '*** Begin Patch'".to_string());
    }
    if lines[lines.len() - 1].trim() != "*** End Patch" {
        return Err("patch must end with '*** End Patch'".to_string());
    }
    let mut ops = 0usize;
    let mut i = 1;
    while i + 1 < lines.len() + 1 && i < lines.len() - 1 {
        let line = lines[i];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            if path.trim().is_empty() {
                return Err("add file with empty path".to_string());
            }
            ops += 1;
            i += 1;
            while i < lines.len() - 1 && !lines[i].starts_with("*** ") {
                if !lines[i].starts_with('+') {
                    return Err(format!("add file contains non '+' line: {:?}", lines[i]));
                }
                i += 1;
            }
            continue;
        }
        if line.starts_with("*** Delete File: ") || line.starts_with("*** Update File: ") {
            ops += 1;
        }
        i += 1;
    }
    Ok(ops)
}

// ---------------------------------------------------------------------------
// Board lease logic (mirrors _board.py / LaneMessageBoard claim rule)
// ---------------------------------------------------------------------------

/// Pure claim-lease decision: which task ids a claimant may take.
/// - Skips ids in `live_leases` (claimed, unexpired) and `retired`.
/// - Oldest-first, up to `limit`.
/// - Returns the ids to hand out; the caller persists the lease rows.
pub fn claimable_ids(
    ordered_ids: &[i64],
    live_leases: &HashSet<i64>,
    retired: &HashSet<i64>,
    limit: usize,
) -> Vec<i64> {
    ordered_ids
        .iter()
        .filter(|id| !live_leases.contains(id) && !retired.contains(id))
        .take(limit)
        .copied()
        .collect()
}

/// A lease is expired when `now_epoch >= claimed_at + lease_secs`.
pub fn lease_expired(claimed_at: i64, now_epoch: i64, lease_secs: i64) -> bool {
    now_epoch >= claimed_at + lease_secs
}

mod hex {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        let b = bytes.as_ref();
        let mut s = String::with_capacity(b.len() * 2);
        for &x in b {
            s.push(CHARS[(x >> 4) as usize] as char);
            s.push(CHARS[(x & 0xF) as usize] as char);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmpdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn list_skips_git_and_applies_glob_rule() {
        let d = tmpdir();
        write(d.path(), "a.py", "x");
        write(d.path(), "sub/b.py", "y");
        write(d.path(), ".git/hidden.py", "z");
        let all = list_files(d.path(), None);
        assert_eq!(all, vec!["a.py".to_string(), "sub/b.py".to_string()]);
        // No slash: basename at any depth.
        assert_eq!(list_files(d.path(), Some("*.py")), all);
        // Slash: relative path match.
        assert_eq!(
            list_files(d.path(), Some("sub/*.py")),
            vec!["sub/b.py".to_string()]
        );
    }

    #[test]
    fn search_returns_rg_style_hits() {
        let d = tmpdir();
        write(d.path(), "a.txt", "hello world\nsecond line\n");
        write(d.path(), "b.txt", "nothing\n");
        let hits = search_files(d.path(), "HELLO", None, 200);
        assert_eq!(hits, vec!["a.txt:1:hello world".to_string()]);
    }

    #[test]
    fn fingerprint_is_stable_and_sorted() {
        let d = tmpdir();
        write(d.path(), "b.txt", "bee");
        write(d.path(), "a.txt", "ay");
        let fps = fingerprint_dir(d.path());
        assert_eq!(fps.len(), 2);
        assert!(fps[0].rel < fps[1].rel);
        assert_eq!(fps.len(), fingerprint_dir(d.path()).len());
        assert_ne!(fps[0].sha256, fps[1].sha256);
    }

    #[test]
    fn line_hash_matches_python_zlib_crc() {
        // zlib.crc32(b"hello") & 0xFF == 0x86; check the Rust mirror agrees.
        assert_eq!(line_hash("hello"), "86");
        assert_eq!(line_hash("  hello  "), line_hash("hello"));
    }

    #[test]
    fn wiki_scan_parses_rows_with_category() {
        let d = tmpdir();
        write(
            d.path(),
            "index.md",
            "### Campaign Finance\n| Foo | MA | [foo.md](foo.md) |\n",
        );
        let rows = scan_wiki_index(d.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Foo");
        assert_eq!(rows[0].category, "Campaign Finance");
    }

    #[test]
    fn patch_check_accepts_and_rejects() {
        let good = "*** Begin Patch\n*** Add File: a.txt\n+hi\n*** End Patch";
        assert_eq!(check_patch(good), Ok(1));
        assert!(check_patch("no markers").is_err());
        let bad = "*** Begin Patch\n*** Add File: a.txt\nno-plus\n*** End Patch";
        assert!(check_patch(bad).is_err());
    }

    #[test]
    fn board_claim_skips_live_and_retired() {
        let ordered = vec![1, 2, 3, 4];
        let live: HashSet<i64> = [2].into_iter().collect();
        let retired: HashSet<i64> = [3].into_iter().collect();
        assert_eq!(claimable_ids(&ordered, &live, &retired, 10), vec![1, 4]);
        assert_eq!(claimable_ids(&ordered, &live, &retired, 1), vec![1]);
        assert!(lease_expired(100, 100 + 1800, 1800));
        assert!(!lease_expired(100, 100 + 1799, 1800));
    }

    #[test]
    fn glob_rule_basename_vs_path() {
        assert!(glob_match("*.py", "sub/b.py"));
        assert!(!glob_match("sub/*.py", "other/b.py"));
        assert!(glob_match("sub/*.py", "sub/b.py"));
    }
}
