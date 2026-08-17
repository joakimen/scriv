//! What you have chosen before, and how much that should count for.
//!
//! A list of two hundred repositories is alphabetical, and the five you are
//! living in this week are scattered through it. This records each selection
//! and floats the ones that are both frequent and recent back to the top —
//! frecency, as Firefox's address bar named it.
//!
//! Everything here is pure: the file read, the file write and the clock live in
//! the [`cmd`](crate::cmd) modules that call it.

use std::path::{Path, PathBuf};

/// One thing that has been chosen, and how its choosing has gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// What was chosen — the value a selector returned, so an absolute path.
    pub key: String,
    /// How many times.
    pub count: u32,
    /// Unix seconds of the most recent time.
    pub when: i64,
}

/// The store, beside the config file and the known-files list.
///
/// A third file in that directory rather than a key in `config.toml`: this one
/// is written by scriv on every selection, and machine writes have no business
/// in a file somebody hand-edits.
pub fn path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("recent")
}

/// How many entries the store keeps. Well past what anyone selects from, and
/// low enough that reading it stays free; the lowest-scoring go first.
const CAPACITY: usize = 500;

/// Parse the store: `<count> <unix-seconds> <key>`, one per line.
///
/// A line that does not parse is skipped rather than refused. Nothing about
/// this file is worth failing a selection over — at worst the ordering is the
/// one it would have had anyway.
pub fn parse(text: &str) -> Vec<Use> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ' ');
            let count = fields.next()?.parse().ok()?;
            let when = fields.next()?.parse().ok()?;
            // The key is last and unsplit: a path may contain spaces.
            let key = fields.next()?;
            (!key.is_empty()).then(|| Use {
                key: key.to_string(),
                count,
                when,
            })
        })
        .collect()
}

/// Render the store back, in the order given.
pub fn render(uses: &[Use]) -> String {
    let mut out = String::new();
    for used in uses {
        // A key holding a newline would come back as two entries, neither of
        // them the path. Nothing produces one, and this is what keeps it so.
        if used.key.contains('\n') {
            continue;
        }
        out.push_str(&format!("{} {} {}\n", used.count, used.when, used.key));
    }
    out
}

/// How long each weight applies for, and what it is worth: a choice made
/// within the hour counts for four of the same choice made last month, and
/// sixteen of one made last year.
///
/// Recency multiplies frequency rather than replacing it, and the multiplier is
/// deliberately small next to the range counts reach. Something opened forty
/// times still outranks something opened twice this morning — it is the better
/// guess, and one selection is all it takes to put the newcomer on top for the
/// rest of the hour.
const WEIGHTS: &[(i64, f64)] = &[
    (60 * 60, 4.0),           // within the hour
    (24 * 60 * 60, 2.0),      // today
    (7 * 24 * 60 * 60, 1.0),  // this week
    (30 * 24 * 60 * 60, 0.5), // this month
];

/// What an older-than-every-weight entry is worth. Not zero: something chosen
/// two hundred times last year is still a better guess than something never
/// chosen at all.
const STALE_WEIGHT: f64 = 0.25;

/// How much this use is worth at `now`.
pub fn score(used: &Use, now: i64) -> f64 {
    let age = now.saturating_sub(used.when).max(0);
    let weight = WEIGHTS
        .iter()
        .find(|(within, _)| age < *within)
        .map(|(_, weight)| *weight)
        .unwrap_or(STALE_WEIGHT);
    f64::from(used.count) * weight
}

/// Record a choice: a new entry, or one more of an existing one.
///
/// Trimmed to [`CAPACITY`] by score, so the file cannot grow without bound —
/// what goes is what was already least likely to be offered first.
pub fn bump(mut uses: Vec<Use>, key: &str, now: i64) -> Vec<Use> {
    match uses.iter_mut().find(|used| used.key == key) {
        Some(used) => {
            used.count = used.count.saturating_add(1);
            used.when = now;
        }
        None => uses.push(Use {
            key: key.to_string(),
            count: 1,
            when: now,
        }),
    }

    if uses.len() > CAPACITY {
        uses.sort_by(|a, b| by_score(a, b, now));
        uses.truncate(CAPACITY);
    }
    uses
}

/// Best first: score, then the more recent of a tie, then by key so the order
/// is the same on every run.
fn by_score(a: &Use, b: &Use, now: i64) -> std::cmp::Ordering {
    score(b, now)
        .partial_cmp(&score(a, now))
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(b.when.cmp(&a.when))
        .then(a.key.cmp(&b.key))
}

/// Reorder `items` so the ones chosen before come first, best guess first.
///
/// Everything unchosen keeps the order it arrived in, below them: the list a
/// command builds is already meaningful — alphabetical, or newest first — and
/// having no history is not a reason to shuffle it.
pub fn order<T>(items: Vec<T>, key: impl Fn(&T) -> &str, uses: &[Use], now: i64) -> Vec<T> {
    if uses.is_empty() {
        return items;
    }

    let mut seen: Vec<(f64, T)> = Vec::new();
    let mut rest: Vec<T> = Vec::new();
    for item in items {
        match uses.iter().find(|used| used.key == key(&item)) {
            Some(used) => seen.push((score(used, now), item)),
            None => rest.push(item),
        }
    }

    // Stable, so two rows of equal score stay in the order the command built
    // them in rather than swapping between runs.
    seen.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let mut out: Vec<T> = seen.into_iter().map(|(_, item)| item).collect();
    out.extend(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60;
    const DAY: i64 = 24 * HOUR;
    const NOW: i64 = 1_785_394_626;

    fn used(key: &str, count: u32, ago: i64) -> Use {
        Use {
            key: key.to_string(),
            count,
            when: NOW - ago,
        }
    }

    #[test]
    fn a_store_round_trips() {
        let uses = vec![
            used("/home/u/dev/scriv", 3, HOUR),
            used("/home/u/a b", 1, 0),
        ];
        assert_eq!(parse(&render(&uses)), uses);
    }

    /// A path may contain spaces, and the key is the rest of the line.
    #[test]
    fn a_key_with_spaces_survives() {
        let parsed = parse("2 100 /home/u/my repo/file.txt\n");
        assert_eq!(parsed[0].key, "/home/u/my repo/file.txt");
        assert_eq!(parsed[0].count, 2);
    }

    #[test]
    fn a_line_that_makes_no_sense_is_skipped_rather_than_fatal() {
        let parsed = parse("garbage\n\n1 notanumber /x\n2 100 /y\n1 200\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].key, "/y");
    }

    #[test]
    fn the_same_count_ranks_by_how_recently_it_was_used() {
        assert!(score(&used("a", 3, HOUR / 2), NOW) > score(&used("b", 3, 3 * DAY), NOW));
        assert!(score(&used("b", 3, 3 * DAY), NOW) > score(&used("c", 3, 300 * DAY), NOW));
    }

    /// The multiplier is small next to the range counts reach, on purpose: a
    /// repository opened forty times is a better guess than one opened twice,
    /// and one selection is all it takes to put the newcomer on top.
    #[test]
    fn frequency_is_not_drowned_out_by_recency() {
        let old_favourite = used("a", 40, 300 * DAY);
        let newcomer = used("b", 2, HOUR / 2);
        assert!(score(&old_favourite, NOW) > score(&newcomer, NOW));

        let bumped = bump(vec![newcomer], "b", NOW);
        assert!(
            score(&bumped[0], NOW) > score(&old_favourite, NOW),
            "one more selection did not put the newcomer on top",
        );
    }

    #[test]
    fn an_old_choice_still_beats_no_choice_at_all() {
        assert!(score(&used("a", 1, 900 * DAY), NOW) > 0.0);
    }

    /// A clock that has gone backwards — a laptop waking with the wrong time,
    /// a store copied between machines — must not make a score enormous.
    #[test]
    fn a_use_from_the_future_is_not_worth_more_than_one_from_now() {
        assert_eq!(
            score(&used("a", 1, -DAY), NOW),
            score(&used("a", 1, 0), NOW)
        );
    }

    #[test]
    fn bumping_counts_a_repeat_and_dates_it() {
        let uses = bump(vec![used("/a", 2, DAY)], "/a", NOW);
        assert_eq!(uses.len(), 1);
        assert_eq!(uses[0].count, 3);
        assert_eq!(uses[0].when, NOW);
    }

    #[test]
    fn bumping_something_new_records_it() {
        let uses = bump(vec![used("/a", 2, DAY)], "/b", NOW);
        assert_eq!(uses.len(), 2);
        assert_eq!(uses[1], used("/b", 1, 0));
    }

    #[test]
    fn the_store_does_not_grow_without_bound() {
        let mut uses: Vec<Use> = (0..CAPACITY)
            .map(|i| used(&format!("/old{i}"), 1, 500 * DAY))
            .collect();
        // One entry worth keeping among a full store of stale ones.
        uses[0] = used("/kept", 50, HOUR);

        let uses = bump(uses, "/new", NOW);
        assert_eq!(uses.len(), CAPACITY);
        assert!(
            uses.iter().any(|u| u.key == "/kept"),
            "dropped the best one"
        );
        assert!(
            uses.iter().any(|u| u.key == "/new"),
            "dropped what it just recorded"
        );
    }

    #[test]
    fn chosen_rows_come_first_and_the_rest_keep_their_order() {
        let items = vec!["/a", "/b", "/c", "/d"];
        let uses = vec![used("/c", 1, HOUR), used("/a", 5, HOUR)];

        let got = order(items, |s| s, &uses, NOW);
        assert_eq!(got, vec!["/a", "/c", "/b", "/d"]);
    }

    #[test]
    fn nothing_chosen_leaves_the_list_exactly_as_it_was() {
        let items = vec!["/a", "/b", "/c"];
        assert_eq!(order(items.clone(), |s| s, &[], NOW), items);
        let unrelated = vec![used("/z", 9, 0)];
        assert_eq!(order(items.clone(), |s| s, &unrelated, NOW), items);
    }

    #[test]
    fn the_store_lives_beside_the_config_it_belongs_to() {
        assert_eq!(
            path(Path::new("/home/u/.config/scriv/config.toml")),
            PathBuf::from("/home/u/.config/scriv/recent")
        );
    }
}
