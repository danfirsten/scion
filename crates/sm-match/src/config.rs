//! The matcher's tuning constants.

use serde::{Deserialize, Serialize};

/// The three GumTree constants, as a config struct rather than as constants.
///
/// SPEC.md §4.3 requires these to be tunable because M5 sweeps them against the
/// mined corpus. They are `serde` round-trippable so that a sweep can write a
/// profile to disk and a regression can be reproduced from a JSON blob.
///
/// # There is no single right profile
///
/// docs/prior-art.md §2.2 records the most valuable single finding of the
/// prior-art survey: **Mergiraf runs the matcher twice with different
/// constants**, and its architecture notes give the reason. A matching that
/// involves `base` should be *permissive*, because a missed match there shows
/// up as a spurious conflict — annoying, never wrong. The `ours`↔`theirs`
/// matching should be *strict*, because a false positive there feeds directly
/// into the merged output, and "any false positive in this matching can have
/// quite detrimental effects on the merged result". SPEC.md §4.3's single set
/// of constants is the wrong shape. Use [`MatchConfig::base_to_side`] and
/// [`MatchConfig::ours_to_theirs`].
///
/// # These numbers are pre-tuning
///
/// Every value below is inherited from someone else's tuning, on someone
/// else's corpus, for a tool with a different emitter. The ASE'14 paper itself
/// says its constants "have been chosen according to our expertise" and that
/// other values could perform differently (docs/prior-art.md §1.1). **M5 sweeps
/// them.** Treat the defaults as a starting point that is known to work, not as
/// a result.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatchConfig {
    /// Minimum subtree height, **1-based**, admitted to the top-down phase.
    ///
    /// A node with no participating children has height 1, so `min_height = 1`
    /// lets bare leaves (identifiers, literals) match top-down and
    /// `min_height = 2` does not. Raising it makes the top-down phase match
    /// fewer, larger, more certain subtrees and hands more work to the
    /// bottom-up phase.
    pub min_height: u32,

    /// Bottom-up dice threshold, compared with a strict `>`.
    ///
    /// `dice(a, b) = 2·|matched descendants shared| / (|desc(a)| + |desc(b)|)`.
    /// Consequences at the extremes, which matter for M5's sweep: `>= 1.0`
    /// disables bottom-up container matching entirely (nothing can exceed 1),
    /// and `0.0` accepts any same-kind candidate that shares even one matched
    /// descendant.
    pub min_dice: f64,

    /// Size ceiling, in participating nodes, for the post-match recovery pass.
    ///
    /// After a bottom-up pair is matched, the recovery pass runs only if both
    /// subtrees hold fewer than this many participating nodes. It bounds the
    /// cost of a pass whose value is concentrated in small subtrees anyway.
    pub max_size: u32,
}

impl MatchConfig {
    /// The permissive profile, for **base↔ours** and **base↔theirs**.
    ///
    /// `min_height = 1`, `min_dice = 0.4`, `max_size = 100` — Mergiraf's
    /// `primary_matcher` (docs/prior-art.md §2.2). A missed match against base
    /// costs a spurious conflict, so this side of the merge should reach.
    ///
    /// One deliberate divergence from Mergiraf: its primary matcher also turns
    /// on RTED (a real tree edit distance) for recovery. We use the cheap
    /// histogram recovery on both profiles — see [`crate::match_trees`] — so
    /// `max_size` bounds a linear pass rather than a cubic one.
    #[must_use]
    pub const fn base_to_side() -> Self {
        Self {
            min_height: 1,
            min_dice: 0.4,
            max_size: 100,
        }
    }

    /// The strict profile, for **ours↔theirs**.
    ///
    /// `min_height = 2`, `min_dice = 0.6`, `max_size = 100` — Mergiraf's
    /// `auxiliary_matcher` (docs/prior-art.md §2.2). This matching only matters
    /// when both branches changed similar things, which is rare; a false
    /// positive in it corrupts the merged result, so it should not reach.
    #[must_use]
    pub const fn ours_to_theirs() -> Self {
        Self {
            min_height: 2,
            min_dice: 0.6,
            max_size: 100,
        }
    }

    /// The ASE'14 paper's constants: `min_height = 2`, `min_dice = 0.5`,
    /// `max_size = 100` (docs/prior-art.md §1.1, §7).
    ///
    /// Not the default for anything. It exists so M5 can report "the paper's
    /// numbers" as a baseline row next to whatever the sweep finds.
    #[must_use]
    pub const fn paper() -> Self {
        Self {
            min_height: 2,
            min_dice: 0.5,
            max_size: 100,
        }
    }
}

impl Default for MatchConfig {
    /// [`MatchConfig::base_to_side`].
    ///
    /// Two of the three matchings in a three-way merge involve base, and a
    /// caller who has not thought about which profile they want is better
    /// served by the one whose failure mode is an extra conflict.
    fn default() -> Self {
        Self::base_to_side()
    }
}

#[cfg(test)]
mod tests {
    use super::MatchConfig;

    #[test]
    fn profiles_have_the_documented_constants() {
        assert_eq!(
            MatchConfig::base_to_side(),
            MatchConfig {
                min_height: 1,
                min_dice: 0.4,
                max_size: 100
            }
        );
        assert_eq!(
            MatchConfig::ours_to_theirs(),
            MatchConfig {
                min_height: 2,
                min_dice: 0.6,
                max_size: 100
            }
        );
        assert_eq!(MatchConfig::default(), MatchConfig::base_to_side());
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = MatchConfig::ours_to_theirs();
        let json = serde_json::to_string(&cfg).expect("serialize");
        let back: MatchConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_fields_fall_back_to_the_default_profile() {
        let cfg: MatchConfig = serde_json::from_str(r#"{"min_dice":0.75}"#).expect("deserialize");
        assert_eq!(
            cfg,
            MatchConfig {
                min_dice: 0.75,
                ..MatchConfig::default()
            }
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let err = serde_json::from_str::<MatchConfig>(r#"{"min_dyce":0.75}"#);
        assert!(err.is_err(), "a typo in a swept config must not be silent");
    }
}
