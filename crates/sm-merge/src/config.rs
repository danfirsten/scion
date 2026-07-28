//! [`MergeConfig`] — the knobs, all of them `serde` round-trippable so that M5
//! can sweep them and a regression can be reproduced from a JSON blob.

use serde::{Deserialize, Serialize};
use sm_cst::Language;
use sm_match::MatchConfig;

/// Tuning for [`crate::merge`].
///
/// The three [`MatchConfig`]s are separate on purpose: matchings against base
/// should be permissive because a miss there costs a spurious conflict, while
/// the ours↔theirs matching should be strict because a false positive there
/// corrupts the output (docs/prior-art.md §2.2). See [`MatchConfig::base_to_side`]
/// and [`MatchConfig::ours_to_theirs`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MergeConfig {
    /// Matcher profile for base↔ours.
    pub base_to_ours: MatchConfig,
    /// Matcher profile for base↔theirs.
    pub base_to_theirs: MatchConfig,
    /// Matcher profile for any direct ours↔theirs matching.
    ///
    /// **The merge does not compute one.** The only question a three-way merge
    /// would ask such a matching is "are these two insertions the same
    /// insertion", and that is answered exactly — by comparing structural
    /// hashes and confirming them with
    /// [`sm_match::structurally_equal`] — rather than approximately. A matcher
    /// can only introduce false positives there, and docs/prior-art.md §2.2
    /// records that a false positive in this matching in particular "can have
    /// quite detrimental effects on the merged result".
    ///
    /// The profile is carried here anyway so that a run's whole configuration
    /// is one serialisable value: `sm diff`'s three-way view and M5's sweep
    /// both want an ours↔theirs matching for *reporting*, and they should not
    /// each invent their own constants.
    pub ours_to_theirs: MatchConfig,

    /// Kinds merged as an **atomic set keyed by their own source text**,
    /// ignoring whatever the matcher paired them with.
    ///
    /// Java's `import_declaration` is the motivating case, and it is the reason
    /// this list exists at all: the matcher's recovery pass will happily pair
    /// `import java.util.HashMap;` with `import java.util.Optional;` because
    /// they are the same shape, and reading that as an *update* is a
    /// semantically wrong merge. Mergiraf marks imports atomic for the same
    /// reason (docs/prior-art.md §2.4).
    ///
    /// Note what is *not* here. TypeScript's `import_statement` is deliberately
    /// left alone: its named-specifier list is itself an unordered container,
    /// so letting the merge descend into it turns "both sides imported one more
    /// name" into a clean set merge, which text keying would turn into a
    /// conflict.
    pub atomic_set_kinds: Vec<String>,

    /// Node kinds a conflict may be promoted to.
    ///
    /// See the crate docs, "Conflict promotion". These are matched exactly;
    /// [`MergeConfig::boundary_suffixes`] covers the rest by name shape.
    pub boundary_kinds: Vec<String>,

    /// Kind-name suffixes that make a node a conflict boundary.
    ///
    /// `_declaration`, `_statement` and `_definition` between them cover every
    /// statement and member kind in both `tree-sitter-java` and
    /// `tree-sitter-typescript` — which is the point: the rule is a property of
    /// how tree-sitter grammars are conventionally named, not a per-language
    /// list that silently rots.
    pub boundary_suffixes: Vec<String>,

    /// Cell budget for the quadratic child-list alignment.
    ///
    /// Above `|base| * |side|` cells the aligner falls back to a
    /// patience-style unique-anchor alignment, which is linearithmic and
    /// strictly weaker. Child lists are tens of elements in real code; this
    /// only matters for machine-generated files.
    pub alignment_budget: usize,

    /// Whether to detect and preserve reparenting moves.
    ///
    /// Off, a node that moved to a different container reads as a delete plus
    /// an insert, which is correct but loses the headline "ours moved it,
    /// theirs edited it" merge. Exists so M5 can measure what move detection
    /// buys.
    pub detect_moves: bool,

    /// Whether an insertion made identically on both sides is emitted once.
    ///
    /// Off, both copies are emitted, which is wrong; this exists only so the
    /// dedup path can be isolated in a test.
    pub deduplicate_insertions: bool,
}

impl MergeConfig {
    /// Kinds treated as an atomic, text-keyed set by default.
    pub const DEFAULT_ATOMIC_SET_KINDS: &'static [&'static str] = &["import_declaration"];

    /// Conflict-boundary kinds that the suffix rule does not catch.
    ///
    /// - `enum_constant` — an enum constant is a declaration in every sense
    ///   that matters here, and it is the element kind of an ordered list, so a
    ///   conflict between two of them must not be promoted past the list.
    /// - `switch_block_statement_group` — a `case` label plus its statements;
    ///   the statements alone are not a coherent region to conflict over.
    /// - `catch_clause` / `finally_clause` — a `catch` cannot be emitted
    ///   without its `try`, but it *can* be conflicted against another `catch`.
    ///
    /// `variable_declarator` is deliberately absent, even though `int a = 1,
    /// b = 2;` makes it tempting: promoting to it turns `int x = 1;` into a
    /// conflict whose ours-block is `x = 2` with the `int` and the `;` outside
    /// the markers, which is not a region anyone can act on. The declaration
    /// above it is.
    pub const DEFAULT_BOUNDARY_KINDS: &'static [&'static str] = &[
        "enum_constant",
        "switch_block_statement_group",
        "catch_clause",
        "finally_clause",
    ];

    /// Kind-name suffixes that make a node a conflict boundary.
    pub const DEFAULT_BOUNDARY_SUFFIXES: &'static [&'static str] =
        &["_declaration", "_statement", "_definition"];

    /// The default profile, with per-language adjustments where any exist.
    ///
    /// There are none today — every default is language-agnostic — but the
    /// entry point exists so that adding one later does not change a signature
    /// every caller uses.
    #[must_use]
    pub fn for_language(_lang: &dyn Language) -> Self {
        Self::default()
    }

    /// Whether a node of this kind may absorb a conflict from below.
    #[must_use]
    pub fn is_boundary(&self, kind: &str) -> bool {
        self.boundary_kinds.iter().any(|k| k == kind)
            || self.boundary_suffixes.iter().any(|s| kind.ends_with(s))
    }

    /// Whether a node of this kind is merged as an atomic, text-keyed element.
    #[must_use]
    pub fn is_atomic_set_kind(&self, kind: &str) -> bool {
        self.atomic_set_kinds.iter().any(|k| k == kind)
    }
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            base_to_ours: MatchConfig::base_to_side(),
            base_to_theirs: MatchConfig::base_to_side(),
            ours_to_theirs: MatchConfig::ours_to_theirs(),
            atomic_set_kinds: Self::DEFAULT_ATOMIC_SET_KINDS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            boundary_kinds: Self::DEFAULT_BOUNDARY_KINDS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            boundary_suffixes: Self::DEFAULT_BOUNDARY_SUFFIXES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            alignment_budget: 250_000,
            detect_moves: true,
            deduplicate_insertions: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MergeConfig;

    #[test]
    fn boundaries_cover_java_and_typescript_statement_kinds() {
        let cfg = MergeConfig::default();
        for kind in [
            "method_declaration",
            "field_declaration",
            "local_variable_declaration",
            "class_declaration",
            "import_declaration",
            "expression_statement",
            "if_statement",
            "return_statement",
            "enum_constant",
            "lexical_declaration",
            "function_declaration",
            "method_definition",
            "public_field_definition",
            "import_statement",
            "export_statement",
        ] {
            assert!(
                cfg.is_boundary(kind),
                "{kind} should be a conflict boundary"
            );
        }
        for kind in [
            "binary_expression",
            "identifier",
            "argument_list",
            "class_body",
            "block",
            "program",
            "formal_parameters",
        ] {
            assert!(!cfg.is_boundary(kind), "{kind} must not absorb conflicts");
        }
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = MergeConfig::default();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: MergeConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(serde_json::from_str::<MergeConfig>(r#"{"detect_move":false}"#).is_err());
    }

    #[test]
    fn partial_json_keeps_the_other_defaults() {
        let cfg: MergeConfig = serde_json::from_str(r#"{"detect_moves":false}"#).expect("parse");
        assert!(!cfg.detect_moves);
        assert_eq!(
            cfg.boundary_suffixes,
            MergeConfig::default().boundary_suffixes
        );
    }
}
