//! Coarse launcher categories — the filter chips over the merged app catalog.
//!
//! The published catalog carries a **fine** per-entry label
//! ([`super::format::AppEntry::category`] — `cards`, `strategy`, `audio`,
//! `utility`, …: ten of them in the shipped corpus) plus the **set** an entry
//! was ingested into (`games` / `apps`, decided by
//! [`super::paths::set_for_type`]). Neither is what a person wants to see as a
//! row of filter buttons: ten chips is a taxonomy, not a filter, and `utility`
//! / `productivity` are publisher words.
//!
//! So there are two levels, and this module owns the fold from one to the
//! other:
//!
//! | coarse chip | fed by |
//! |---|---|
//! | `games`  | the whole `games` **set**, whatever its fine category |
//! | `music`  | fine `audio`, `music` |
//! | `art`    | fine `art` |
//! | `tools`  | fine `utility`, `productivity` |
//! | `other`  | anything else, including an entry with no category at all |
//!
//! **Set wins over category for games, deliberately.** A rhythm game published
//! into the `games` set with `category: "music"` is a game — filing it under
//! Music because of its fine label would hide it from the chip a person
//! actually presses to find games. The fine label is not lost: it is what a
//! future *sub*-chip row inside Games would be built from
//! ([`fine_labels_in`]).
//!
//! **A chip that would show an empty grid is not rendered** ([`chips_for`]).
//! An empty chip is indistinguishable from a broken filter — you press Music,
//! get nothing, and cannot tell whether the deployment has no music apps or the
//! filter is broken. Counts ride along for the same reason.

use super::format::AppEntry;
use super::paths;

/// The coarse chip an entry belongs to. Stable keys — persisted as the selected
/// filter in window view-state and used as the i18n key stem
/// (`apps.filter.<key>`), so renaming one is a migration, not a relabel.
pub const GAMES: &str = "games";
pub const MUSIC: &str = "music";
pub const ART: &str = "art";
pub const TOOLS: &str = "tools";
pub const OTHER: &str = "other";

/// The pseudo-chip meaning "no filter". Never produced by [`coarse_for`] — it
/// is the default selection and the first chip rendered.
pub const ALL: &str = "all";

/// Display order of the coarse chips. `other` is last because it is a
/// remainder, not a category.
pub const COARSE_ORDER: &[&str] = &[GAMES, MUSIC, ART, TOOLS, OTHER];

/// Which coarse chip an entry falls under. See the module table.
pub fn coarse_for(set: &str, category: Option<&str>) -> &'static str {
    if set == paths::GAMES_SET {
        return GAMES;
    }
    match category.unwrap_or("") {
        "audio" | "music" => MUSIC,
        "art" => ART,
        "utility" | "productivity" => TOOLS,
        _ => OTHER,
    }
}

/// One rendered filter chip: its key and how many entries it would show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chip {
    pub key: &'static str,
    pub count: usize,
}

/// The chips to render for a catalog, in [`COARSE_ORDER`] with [`ALL`] first.
///
/// Two suppressions, both so the row can never lie about what pressing it does:
/// a coarse chip with **no** entries is omitted, and when only **one** coarse
/// chip survives the row is dropped entirely (a single chip beside "All" filters
/// nothing — it is chrome claiming to be a control).
pub fn chips_for<'a, I>(entries: I) -> Vec<Chip>
where
    I: IntoIterator<Item = (&'a str, &'a AppEntry)>,
{
    let mut counts: Vec<(&'static str, usize)> =
        COARSE_ORDER.iter().map(|k| (*k, 0usize)).collect();
    let mut total = 0usize;
    for (set, entry) in entries {
        total += 1;
        let key = coarse_for(set, entry.category.as_deref());
        if let Some(slot) = counts.iter_mut().find(|(k, _)| *k == key) {
            slot.1 += 1;
        }
    }
    let present: Vec<Chip> = counts
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(key, count)| Chip { key, count })
        .collect();
    if present.len() < 2 {
        return Vec::new();
    }
    let mut out = vec![Chip {
        key: ALL,
        count: total,
    }];
    out.extend(present);
    out
}

/// Whether an entry passes the currently-selected chip. [`ALL`] and any
/// unrecognized selection pass everything — a selection persisted before a chip
/// existed (or after one was renamed) must degrade to "show me everything",
/// never to an empty grid the user cannot explain.
pub fn passes(selected: &str, set: &str, category: Option<&str>) -> bool {
    if selected.is_empty() || selected == ALL || !COARSE_ORDER.contains(&selected) {
        return true;
    }
    coarse_for(set, category) == selected
}

/// The distinct **fine** labels present under one coarse chip, in first-seen
/// order. Not rendered yet — this is the seam a sub-chip row inside Games
/// (`cards` · `strategy` · `puzzle` · …) is built from, kept here so the two
/// levels stay in one module rather than growing a second taxonomy in the view.
#[allow(dead_code)]
pub fn fine_labels_in<'a, I>(entries: I, coarse: &str) -> Vec<String>
where
    I: IntoIterator<Item = (&'a str, &'a AppEntry)>,
{
    let mut out: Vec<String> = Vec::new();
    for (set, entry) in entries {
        if coarse_for(set, entry.category.as_deref()) != coarse {
            continue;
        }
        if let Some(c) = entry.category.as_deref().filter(|c| !c.is_empty()) {
            if !out.iter().any(|s| s == c) {
                out.push(c.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, category: Option<&str>) -> AppEntry {
        AppEntry {
            id: id.to_string(),
            name: id.to_string(),
            category: category.map(str::to_string),
            ..Default::default()
        }
    }

    /// The fold, checked against the ten fine labels the shipped corpus
    /// actually publishes (measured 2026-08-19 across 33 apps) rather than
    /// against an invented set — a mapping table is only worth anything if its
    /// left column is real.
    #[test]
    fn the_shipped_corpus_folds_into_the_four_chips() {
        // games set — every fine label there lands under Games.
        for c in ["cards", "strategy", "puzzle", "arcade", "word"] {
            assert_eq!(coarse_for(paths::GAMES_SET, Some(c)), GAMES, "games/{c}");
        }
        // apps set — the fine labels the corpus uses.
        assert_eq!(coarse_for(paths::APPS_SET, Some("audio")), MUSIC);
        assert_eq!(coarse_for(paths::APPS_SET, Some("music")), MUSIC);
        assert_eq!(coarse_for(paths::APPS_SET, Some("art")), ART);
        assert_eq!(coarse_for(paths::APPS_SET, Some("utility")), TOOLS);
        assert_eq!(coarse_for(paths::APPS_SET, Some("productivity")), TOOLS);
    }

    /// A game keeps its chip whatever its fine label says. This is the rule the
    /// module doc calls deliberate; without it a publisher's `category:"music"`
    /// on a rhythm game would move it out of Games.
    #[test]
    fn the_set_decides_a_game_not_its_fine_label() {
        assert_eq!(coarse_for(paths::GAMES_SET, Some("music")), GAMES);
        assert_eq!(coarse_for(paths::GAMES_SET, Some("art")), GAMES);
        assert_eq!(coarse_for(paths::GAMES_SET, None), GAMES);
    }

    /// An app with an unknown or absent category is filed, not dropped — it
    /// still appears under All, and under Other rather than silently nowhere.
    #[test]
    fn an_unknown_category_lands_in_other_never_nowhere() {
        assert_eq!(coarse_for(paths::APPS_SET, Some("holography")), OTHER);
        assert_eq!(coarse_for(paths::APPS_SET, None), OTHER);
        let e = entry("x", Some("holography"));
        let chips = chips_for([(paths::APPS_SET, &e), (paths::GAMES_SET, &e)]);
        assert!(chips.iter().any(|c| c.key == OTHER && c.count == 1));
        assert!(passes(ALL, paths::APPS_SET, Some("holography")));
        assert!(passes(OTHER, paths::APPS_SET, Some("holography")));
    }

    /// A chip whose grid would be empty must not render — pressing it would be
    /// indistinguishable from a broken filter.
    #[test]
    fn an_empty_chip_is_not_rendered_and_counts_are_real() {
        let cards = entry("war", Some("cards"));
        let synth = entry("synth", Some("audio"));
        let radio = entry("radio", Some("audio"));
        let chips = chips_for([
            (paths::GAMES_SET, &cards),
            (paths::APPS_SET, &synth),
            (paths::APPS_SET, &radio),
        ]);
        let keys: Vec<&str> = chips.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec![ALL, GAMES, MUSIC], "art/tools/other are empty");
        assert_eq!(chips[0].count, 3, "All counts every entry");
        assert_eq!(chips[1].count, 1);
        assert_eq!(chips[2].count, 2);
    }

    /// One surviving chip means there is nothing to filter, so the row is
    /// dropped whole rather than rendering "All · Games" — two buttons that
    /// show the same grid.
    #[test]
    fn a_single_category_renders_no_chip_row_at_all() {
        let a = entry("war", Some("cards"));
        let b = entry("chess", Some("strategy"));
        assert!(chips_for([(paths::GAMES_SET, &a), (paths::GAMES_SET, &b)]).is_empty());
        assert!(chips_for(std::iter::empty()).is_empty());
    }

    /// A stale or unknown persisted selection shows everything. The failure it
    /// prevents is a user reopening the launcher to an empty grid after a chip
    /// key changed, with no way to work out why.
    #[test]
    fn an_unknown_selection_degrades_to_showing_everything() {
        assert!(passes("retro-arcade", paths::APPS_SET, Some("audio")));
        assert!(passes("", paths::GAMES_SET, Some("cards")));
        assert!(!passes(MUSIC, paths::GAMES_SET, Some("cards")));
    }

    /// The sub-chip seam: fine labels under a coarse chip, deduped, in
    /// first-seen order, and only for that chip.
    #[test]
    fn fine_labels_are_the_sub_chip_seam() {
        let cards = entry("war", Some("cards"));
        let strat = entry("chess", Some("strategy"));
        let cards2 = entry("solitaire", Some("cards"));
        let audio = entry("radio", Some("audio"));
        let all = [
            (paths::GAMES_SET, &cards),
            (paths::GAMES_SET, &strat),
            (paths::GAMES_SET, &cards2),
            (paths::APPS_SET, &audio),
        ];
        assert_eq!(fine_labels_in(all, GAMES), vec!["cards", "strategy"]);
        assert_eq!(fine_labels_in(all, MUSIC), vec!["audio"]);
    }
}
