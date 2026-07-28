//! A byte-offset → line-number index over a source buffer.
//!
//! `sm-cst`'s trivia pass has one of these too, but it is private to that crate
//! and answers a different question (how many newlines lie in a gap). This one
//! exists to turn a node's byte range into the numbered source lines a diff
//! viewer prints, and to hand back the raw bytes of a line without decoding the
//! whole file — the source is `Vec<u8>` and may not be UTF-8 (`sm-cst`
//! decision 4), so display goes through `from_utf8_lossy` one line at a time.

/// Offsets of the first byte of every line, plus a sentinel at `len`.
#[derive(Clone, Debug)]
pub struct LineIndex {
    starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    /// Build the index in one pass.
    ///
    /// Lines are delimited by `\n` only, matching `sm-cst`'s trivia policy
    /// (PROGRESS.md decision 24): `\r\n` is one break and a lone `\r` is not a
    /// break at all.
    #[must_use]
    pub fn new(source: &[u8]) -> Self {
        let mut starts = vec![0u32];
        for (i, &b) in source.iter().enumerate() {
            if b == b'\n' {
                starts.push(i as u32 + 1);
            }
        }
        Self {
            starts,
            len: source.len() as u32,
        }
    }

    /// Total number of lines. An empty buffer has one (empty) line.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.starts.len()
    }

    /// The **1-based** line number containing `offset`.
    #[must_use]
    pub fn line_of(&self, offset: u32) -> usize {
        self.starts.partition_point(|&s| s <= offset)
    }

    /// The 1-based line numbers a half-open byte range touches, inclusive.
    ///
    /// A range that ends exactly at a line start (a node whose last byte is the
    /// newline before it) does not claim that next line. A zero-length range
    /// claims the single line it sits on.
    #[must_use]
    pub fn lines_of(&self, range: std::ops::Range<u32>) -> std::ops::RangeInclusive<usize> {
        let first = self.line_of(range.start);
        let last = if range.end > range.start {
            self.line_of(range.end - 1)
        } else {
            first
        };
        first..=last
    }

    /// Half-open byte range of the 1-based line `n`, its `\n` excluded.
    ///
    /// Every entry of `starts` past the first sits one byte after a `\n`, so
    /// the terminator can be dropped without looking at the source again.
    #[must_use]
    pub fn line_span(&self, n: usize) -> std::ops::Range<u32> {
        let start = self.starts[n - 1];
        let end = self.starts.get(n).map_or(self.len, |&next| next - 1);
        start..end
    }

    /// The text of the 1-based line `n`, lossily decoded, without its
    /// terminator (`\r\n` and `\n` both come back bare).
    #[must_use]
    pub fn line_text<'a>(&self, source: &'a [u8], n: usize) -> std::borrow::Cow<'a, str> {
        let span = self.line_span(n);
        let mut bytes = &source[span.start as usize..span.end as usize];
        if let [rest @ .., b'\r'] = bytes {
            bytes = rest;
        }
        String::from_utf8_lossy(bytes)
    }
}

/// Number of leading whitespace bytes in `line`.
#[must_use]
pub fn indent_width(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

#[cfg(test)]
mod tests {
    use super::LineIndex;

    #[test]
    fn line_numbers_are_one_based() {
        let src = b"alpha\nbeta\ngamma";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.line_of(0), 1);
        assert_eq!(idx.line_of(5), 1); // the newline itself belongs to line 1
        assert_eq!(idx.line_of(6), 2);
        assert_eq!(idx.line_of(11), 3);
    }

    #[test]
    fn a_range_ending_on_a_newline_does_not_claim_the_next_line() {
        let src = b"alpha\nbeta\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.lines_of(0..6), 1..=1);
        assert_eq!(idx.lines_of(0..7), 1..=2);
        assert_eq!(idx.lines_of(3..3), 1..=1);
    }

    #[test]
    fn line_text_drops_the_terminator() {
        let src = b"alpha\r\nbeta\n";
        let idx = LineIndex::new(src);
        assert_eq!(idx.line_text(src, 1), "alpha");
        assert_eq!(idx.line_text(src, 2), "beta");
        assert_eq!(idx.line_text(src, 3), "");
    }

    #[test]
    fn an_empty_buffer_has_one_empty_line() {
        let idx = LineIndex::new(b"");
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.line_text(b"", 1), "");
    }
}
