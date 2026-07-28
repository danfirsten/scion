//! Small pure helpers: hashing, slugs, contamination.
//!
//! Everything here is deterministic by construction. Nothing in the corpus may
//! depend on hash-map iteration order, on `DefaultHasher` (whose algorithm is
//! explicitly not stable across Rust releases), or on the wall clock — a second
//! run over the same repository state must produce byte-identical output.

/// FNV-1a, 64-bit. Chosen over `DefaultHasher` precisely because it is a fixed
/// algorithm: the clean-case sample must not shift when the toolchain moves.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Turn a repository path into something safe to use as a directory name.
///
/// Long Java paths are common, and some filesystems cap a single component at
/// 255 bytes, so an over-long slug keeps its *tail* (which is the informative
/// end) and gains a hash of the full path to stay collision-free.
pub fn path_slug(path: &str) -> String {
    let mut s: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Every char above maps to ASCII, so byte length and char count agree and
    // slicing cannot split a code point.
    if s.len() > 120 {
        let tail = &s[s.len() - 100..];
        s = format!("{tail}-{:016x}", fnv1a64(path.as_bytes()));
    }
    s
}

/// `<12-char merge sha>-<path slug>`.
pub fn case_id(merge_sha: &str, path: &str) -> String {
    let short: String = merge_sha.chars().take(12).collect();
    format!("{short}-{}", path_slug(path))
}

/// `owner/repo` inferred from a clone URL, e.g.
/// `https://github.com/apache/kafka.git` -> `apache/kafka`.
pub fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed);
    let parts: Vec<&str> = trimmed.rsplit('/').take(2).collect();
    match parts.len() {
        2 => format!("{}/{}", parts[1], parts[0]),
        1 => parts[0].to_string(),
        _ => trimmed.to_string(),
    }
}

/// A corpus directory name for a repository: `owner__repo`.
pub fn repo_dir_name(name: &str) -> String {
    name.replace('/', "__")
}

/// Heuristic "this is not source code we can merge".
///
/// A NUL byte anywhere is git's own binary test (git looks at the first 8000
/// bytes; we look at all of them, which can only be more conservative).
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Count lines the way the contamination filter does.
pub fn line_count(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    let n = bytes.iter().filter(|b| **b == b'\n').count() as u64;
    if bytes.last() == Some(&b'\n') {
        n
    } else {
        n + 1
    }
}

/// Result of the SPEC §6.1 contamination check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contamination {
    /// Lines of the resolution present in none of base/ours/theirs.
    pub count: u64,
    /// Up to `SAMPLE_LIMIT` of them, verbatim (minus trailing whitespace).
    pub sample: Vec<String>,
}

impl Contamination {
    pub const SAMPLE_LIMIT: usize = 8;

    pub fn contaminated(&self) -> bool {
        self.count > 0
    }
}

/// SPEC §6.1: flag a resolution that contains a line present in no input.
///
/// Lines are compared with **trailing** whitespace trimmed and leading
/// whitespace kept: reindentation is a real edit that the merge has to
/// reproduce, whereas a stripped trailing space is an editor artefact that
/// would otherwise mark half the corpus contaminated.
///
/// This is a *superset* test. A human who reformatted one line, bumped a
/// copyright year or resolved by writing genuinely new code all land here
/// together; the count and the sample are what let M5 tell them apart.
pub fn contamination(resolved: &[u8], inputs: &[&[u8]]) -> Contamination {
    use std::collections::HashSet;

    let mut known: HashSet<&[u8]> = HashSet::new();
    for input in inputs {
        for line in split_lines(input) {
            known.insert(trim_end(line));
        }
    }

    let mut count = 0u64;
    let mut sample = Vec::new();
    for line in split_lines(resolved) {
        let line = trim_end(line);
        if !known.contains(line) {
            count += 1;
            if sample.len() < Contamination::SAMPLE_LIMIT {
                sample.push(String::from_utf8_lossy(line).into_owned());
            }
        }
    }
    Contamination { count, sample }
}

/// Lines of a byte slice, agreeing with [`line_count`]: a trailing newline
/// terminates the last line rather than starting an empty one, and an empty
/// input has no lines at all.
fn split_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let trimmed = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    // `take(0)` is how the empty input yields nothing without boxing the
    // iterator or allocating a `Vec`.
    let limit = if bytes.is_empty() { 0 } else { usize::MAX };
    trimmed.split(|b| *b == b'\n').take(limit)
}

fn trim_end(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] as char).is_ascii_whitespace() {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_stable_and_bounded() {
        assert_eq!(
            path_slug("src/main/java/A.java"),
            "src_main_java_A.java".to_string()
        );
        let long = format!("a/{}/B.java", "x".repeat(300));
        let s = path_slug(&long);
        assert!(s.len() <= 120, "slug was {} bytes", s.len());
        assert_eq!(s, path_slug(&long));
        assert_ne!(s, path_slug(&format!("{long}x")));
    }

    #[test]
    fn case_ids_shorten_the_sha() {
        assert_eq!(
            case_id("0123456789abcdef0123456789abcdef01234567", "a/B.java"),
            "0123456789ab-a_B.java"
        );
    }

    #[test]
    fn repo_names_come_off_the_url() {
        assert_eq!(
            repo_name_from_url("https://github.com/apache/kafka.git"),
            "apache/kafka"
        );
        assert_eq!(
            repo_name_from_url("https://github.com/apache/kafka/"),
            "apache/kafka"
        );
        assert_eq!(repo_dir_name("apache/kafka"), "apache__kafka");
    }

    #[test]
    fn counts_lines_with_and_without_a_final_newline() {
        assert_eq!(line_count(b""), 0);
        assert_eq!(line_count(b"a"), 1);
        assert_eq!(line_count(b"a\n"), 1);
        assert_eq!(line_count(b"a\nb"), 2);
        assert_eq!(line_count(b"a\nb\n"), 2);
    }

    #[test]
    fn contamination_finds_only_novel_lines() {
        let base = b"a\nb\nc\n";
        let ours = b"a\nB\nc\n";
        let theirs = b"a\nb\nC\n";

        let clean = contamination(b"a\nB\nC\n", &[base, ours, theirs]);
        assert_eq!(clean.count, 0);
        assert!(!clean.contaminated());

        let dirty = contamination(b"a\nB\nC\nnovel();\n", &[base, ours, theirs]);
        assert_eq!(dirty.count, 1);
        assert_eq!(dirty.sample, vec!["novel();".to_string()]);
    }

    #[test]
    fn trailing_whitespace_is_not_contamination() {
        let ours = b"int x = 1;\n";
        let c = contamination(b"int x = 1;   \n", &[ours]);
        assert_eq!(c.count, 0);
    }

    #[test]
    fn leading_whitespace_is_contamination() {
        // Reindentation is a real edit and the merge has to reproduce it.
        let ours = b"int x = 1;\n";
        let c = contamination(b"    int x = 1;\n", &[ours]);
        assert_eq!(c.count, 1);
    }

    #[test]
    fn fnv_is_the_documented_constant() {
        // Known-answer test: FNV-1a/64 of "a".
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
