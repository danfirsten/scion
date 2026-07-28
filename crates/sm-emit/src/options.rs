//! [`EmitOptions`] and the conflict-marker style.

use serde::{Deserialize, Serialize};

/// How to render a conflict.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStyle {
    /// `<<<<<<<` / `=======` / `>>>>>>>`. Git's `merge` style.
    #[default]
    Merge,
    /// Git's `diff3` style: the ancestor's version is included between
    /// `|||||||` and `=======`.
    ///
    /// Worth defaulting to for a *structural* merge in particular. Our
    /// conflicts are promoted to whole statements and declarations, which is
    /// good for coherence and bad for pinpointing, and the ancestor block is
    /// what lets a reader see which of the two sides actually moved.
    Diff3,
}

/// Everything the emitter needs beyond the merged tree.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmitOptions {
    /// Number of `<`, `|`, `=` and `>` characters in a marker line.
    ///
    /// Git passes this to a merge driver as `%L` and it is not decoration: a
    /// file that legitimately contains a seven-character marker line is merged
    /// with a longer one so the markers stay unambiguous. Values below
    /// [`EmitOptions::MIN_MARKER_SIZE`] are clamped up.
    pub marker_size: usize,
    /// Label on the `<<<<<<<` line. Git passes this as `%X`.
    pub ours_label: String,
    /// Label on the `>>>>>>>` line. Git passes this as `%Y`.
    pub theirs_label: String,
    /// Label on the `|||||||` line, in [`ConflictStyle::Diff3`]. Git passes
    /// this as `%S`.
    pub base_label: String,
    /// Marker style.
    pub style: ConflictStyle,
    /// Whether to reindent a spliced subtree whose nesting depth changed.
    ///
    /// On by default (SPEC.md §4.6). Off, output stays a pure byte splice,
    /// which is useful for isolating the reindenter in tests and for anyone who
    /// would rather have wrong indentation than a transformed byte.
    pub reindent: bool,
}

impl EmitOptions {
    /// Git's default marker size, and the smallest this emitter will use.
    pub const MIN_MARKER_SIZE: usize = 7;

    /// The effective marker size.
    #[must_use]
    pub fn effective_marker_size(&self) -> usize {
        self.marker_size.max(Self::MIN_MARKER_SIZE)
    }

    /// Set the labels git supplies as `%X` / `%Y` / `%S`.
    ///
    /// Git older than 2.44 does not expand `%S`, so the literal string `"%S"`
    /// arrives as the argument; the driver detects that (docs/prior-art.md
    /// §2.9) and simply does not call this.
    #[must_use]
    pub fn with_labels(
        mut self,
        ours: impl Into<String>,
        theirs: impl Into<String>,
        base: impl Into<String>,
    ) -> Self {
        self.ours_label = ours.into();
        self.theirs_label = theirs.into();
        self.base_label = base.into();
        self
    }
}

impl Default for EmitOptions {
    fn default() -> Self {
        Self {
            marker_size: Self::MIN_MARKER_SIZE,
            ours_label: "ours".to_owned(),
            theirs_label: "theirs".to_owned(),
            base_label: "base".to_owned(),
            style: ConflictStyle::Merge,
            reindent: true,
        }
    }
}

/// What [`crate::emit`] produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EmitResult {
    /// The merged file.
    pub bytes: Vec<u8>,
    /// How many conflict-marker regions were written.
    pub conflict_count: usize,
    /// How many bytes came from a `Gap::Synthesized` rather than an input.
    ///
    /// Conflict markers are *not* counted here: they are a deliberate, visible
    /// annotation, not silently invented source. This counter is for SPEC.md
    /// §5's byte-preservation property, which is about text a reader would
    /// mistake for their own code.
    pub synthesized_bytes: usize,
    /// Of [`EmitResult::synthesized_bytes`], how many were single spaces the
    /// emitter inserted to stop two adjacent tokens lexing as one.
    ///
    /// Counted separately because the two are not the same kind of event. A
    /// [`sm_merge::Gap::Synthesized`] is invented *layout*; a separator here is
    /// the emitter refusing to write `staticint`, and it is only ever one
    /// U+0020 between two tokens that came from real inputs. The driver's
    /// self-check uses the distinction: `synthesized_bytes ==
    /// synthesized_separators` still means "a pure splice, plus the separators
    /// the lexical invariant required". See `crate::emit`'s docs.
    ///
    /// Normally zero: `sm-merge` repairs the gap it copies (its crate docs, §9)
    /// and this backstop never fires. It is not zero *by construction*, which
    /// is why it is reported rather than asserted.
    pub synthesized_separators: usize,
    /// How many lines had their leading whitespace rewritten by the
    /// reindenter.
    pub reindented_lines: usize,
}

#[cfg(test)]
mod tests {
    use super::{ConflictStyle, EmitOptions};

    #[test]
    fn marker_size_is_clamped_to_gits_minimum() {
        let opts = EmitOptions {
            marker_size: 3,
            ..EmitOptions::default()
        };
        assert_eq!(opts.effective_marker_size(), 7);
        let opts = EmitOptions {
            marker_size: 11,
            ..EmitOptions::default()
        };
        assert_eq!(opts.effective_marker_size(), 11);
    }

    #[test]
    fn options_round_trip_through_json() {
        let opts = EmitOptions {
            style: ConflictStyle::Diff3,
            ..EmitOptions::default()
        }
        .with_labels("HEAD", "feature", "merged common ancestors");
        let json = serde_json::to_string(&opts).expect("serialize");
        assert_eq!(
            serde_json::from_str::<EmitOptions>(&json).expect("deserialize"),
            opts
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<EmitOptions>(r#"{"markersize":7}"#).is_err());
    }
}
