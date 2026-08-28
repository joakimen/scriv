//! Rendering a run of project steps for a terminal.
//!
//! Everything here is pure: [`crate::cmd::project`] runs the steps and prints
//! what comes back. Colour is the resolved [`crate::Ctx::color`], never a
//! terminal check of its own.

use std::time::Duration;

use crate::term::paint;

use super::Step;

/// How a step ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Done,
    /// The tool is not installed, so there was nothing to run. Not a failure:
    /// a project that pins a toolchain the machine does not have is a machine
    /// to install it on, not a broken run.
    Skipped {
        reason: String,
    },
    Failed {
        code: Option<i32>,
    },
}

/// One finished step.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub name: &'static str,
    pub status: Status,
    /// Both of the command's streams, empty when its output was not captured.
    pub output: String,
    pub duration: Duration,
}

impl Outcome {
    pub fn failed(&self) -> bool {
        matches!(self.status, Status::Failed { .. })
    }
}

/// Colours cycled in plan order, so one step is told from another at a glance.
/// Red is left to failures, and these are the terminal's own palette, as the
/// rest of scriv's colouring is.
const STEP_COLORS: &[u8] = &[6, 5, 2, 3, 4];

/// The colour of the step at `index` in the plan.
pub fn color(index: usize) -> u8 {
    STEP_COLORS[index % STEP_COLORS.len()]
}

/// Grey, for everything a row says about itself rather than names.
const DIM: u8 = 8;
const GREEN: u8 = 2;
const RED: u8 = 1;

/// One step's line, written as it finishes. `width` is the widest step name in
/// the run, so the second column lines up however the steps interleave.
pub fn status_line(outcome: &Outcome, width: usize, color: bool) -> String {
    let name = pad(outcome.name, width);
    match &outcome.status {
        Status::Done => format!(
            "{} {name}  {}",
            paint("✓", GREEN, color),
            paint(&format_duration(outcome.duration), DIM, color)
        ),
        Status::Skipped { reason } => format!(
            "{} {}  {}",
            paint("-", DIM, color),
            paint(&name, DIM, color),
            paint(reason, DIM, color)
        ),
        Status::Failed { code } => format!(
            "{} {name}  {}",
            paint("✗", RED, color),
            paint(&failure_reason(*code), RED, color)
        ),
    }
}

fn failure_reason(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("exit {code}"),
        None => "did not run".to_string(),
    }
}

/// The width the labels in [`summary`] are padded to, wide enough for the
/// longest of them and a gap.
const LABEL_WIDTH: usize = 11;

/// What the run came to: the steps that did not finish, then the wall time.
/// Both are named in plan order, since that is the order they were listed in.
pub fn summary(outcomes: &[Outcome], elapsed: Duration, color: bool) -> Vec<String> {
    let mut lines = Vec::new();
    let mut group = |label: &str, tint: u8, entries: Vec<String>| {
        if !entries.is_empty() {
            lines.push(format!(
                "{}{}",
                paint(&pad(label, LABEL_WIDTH), tint, color),
                entries.join(", ")
            ));
        }
    };

    group(
        "skipped",
        DIM,
        collect(outcomes, |outcome| match &outcome.status {
            Status::Skipped { reason } => Some(format!("{} ({reason})", outcome.name)),
            _ => None,
        }),
    );
    group(
        "failed",
        RED,
        collect(outcomes, |outcome| {
            outcome.failed().then(|| outcome.name.to_string())
        }),
    );
    lines.push(format!(
        "{}{}",
        paint(&pad("total", LABEL_WIDTH), DIM, color),
        paint(&format_duration(elapsed), DIM, color)
    ));

    lines
}

fn collect(outcomes: &[Outcome], of: impl Fn(&Outcome) -> Option<String>) -> Vec<String> {
    outcomes.iter().filter_map(of).collect()
}

/// The `[name]` each streamed line is written behind: coloured per step, and
/// padded so every step's output starts in the same column.
pub fn prefixes(names: &[&str], color: bool) -> Vec<String> {
    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0)
        + 3;

    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let label = format!("[{name}]");
            // The padding stays outside the colour: a trailing run of coloured
            // spaces is a background nobody asked for.
            format!(
                "{}{}",
                paint(&label, self::color(index), color),
                " ".repeat(width.saturating_sub(label.chars().count()))
            )
        })
        .collect()
}

/// The plan as a dry run prints it: each step, the files that selected it, and
/// the command it would run. Each step keeps the colour its streamed output is
/// prefixed with, so it looks the same in both modes.
pub fn plan_listing(steps: &[&Step], color: bool) -> Vec<String> {
    let name_width = width(steps.iter().map(|step| step.name));
    let evidence_width = width(steps.iter().map(|step| step.evidence.as_str()));

    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            format!(
                "{}  {}  {} {}",
                paint(&pad(step.name, name_width), self::color(index), color),
                paint(&pad(&step.evidence, evidence_width), DIM, color),
                paint("$", DIM, color),
                step.command_line()
            )
            .trim_end()
            .to_string()
        })
        .collect()
}

fn width<'a>(texts: impl Iterator<Item = &'a str>) -> usize {
    texts.map(|text| text.chars().count()).max().unwrap_or(0)
}

/// Pad to `width` columns, counting characters rather than bytes.
pub fn pad(text: &str, width: usize) -> String {
    let mut out = text.to_string();
    out.push_str(&" ".repeat(width.saturating_sub(text.chars().count())));
    out
}

/// Milliseconds under a second, one decimal of a second above it. A step that
/// takes minutes is still reported in seconds: it is a number to compare with
/// the step beside it, not a clock.
pub fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(name: &'static str, status: Status) -> Outcome {
        Outcome {
            name,
            status,
            output: String::new(),
            duration: Duration::from_millis(120),
        }
    }

    fn skipped(reason: &str) -> Status {
        Status::Skipped {
            reason: reason.to_string(),
        }
    }

    #[test]
    fn durations_switch_from_milliseconds_to_seconds() {
        assert_eq!(format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
        assert_eq!(format_duration(Duration::from_millis(1000)), "1.0s");
        assert_eq!(format_duration(Duration::from_millis(4321)), "4.3s");
    }

    #[test]
    fn a_status_line_says_which_of_the_three_things_happened_without_colour() {
        let done = status_line(&outcome("rust", Status::Done), 6, false);
        let skip = status_line(&outcome("maven", skipped("mvn not found")), 6, false);
        let fail = status_line(&outcome("go", Status::Failed { code: Some(1) }), 6, false);

        assert_eq!(done, "✓ rust    120ms");
        assert_eq!(skip, "- maven   mvn not found");
        assert_eq!(fail, "✗ go      exit 1");
    }

    #[test]
    fn a_step_that_never_started_says_so_rather_than_naming_a_status() {
        assert_eq!(failure_reason(None), "did not run");
        assert_eq!(failure_reason(Some(2)), "exit 2");
    }

    #[test]
    fn the_summary_names_skips_and_failures_in_plan_order() {
        let outcomes = [
            outcome("mise", Status::Done),
            outcome("maven", skipped("mvn not found")),
            outcome("go", Status::Failed { code: Some(1) }),
            outcome("rust", Status::Done),
            outcome("deno", skipped("deno not found")),
        ];

        assert_eq!(
            summary(&outcomes, Duration::from_millis(3500), false),
            vec![
                "skipped    maven (mvn not found), deno (deno not found)",
                "failed     go",
                "total      3.5s",
            ]
        );
    }

    #[test]
    fn a_run_with_nothing_to_report_is_the_total_alone() {
        let outcomes = [outcome("rust", Status::Done)];
        assert_eq!(
            summary(&outcomes, Duration::from_millis(12), false),
            vec!["total      12ms"]
        );
    }

    #[test]
    fn every_step_gets_a_prefix_of_the_same_width() {
        let names = ["mise", "rust", "terraform"];
        let prefixes = prefixes(&names, false);

        assert_eq!(prefixes.len(), names.len());
        let width = prefixes[0].chars().count();
        for (prefix, name) in prefixes.iter().zip(names) {
            assert!(prefix.starts_with(&format!("[{name}]")), "{prefix}");
            assert_eq!(prefix.chars().count(), width, "{prefix}");
        }
    }

    #[test]
    fn the_plan_listing_shows_the_name_the_evidence_and_the_command() {
        let steps = [
            Step::new("mise", "mise.toml".into(), "mise", &["install"]),
            Step::new("bun", "package.json + bun.lock".into(), "bun", &["install"]),
        ];
        let refs: Vec<&Step> = steps.iter().collect();

        assert_eq!(
            plan_listing(&refs, false),
            vec![
                "mise  mise.toml                $ mise install",
                "bun   package.json + bun.lock  $ bun install",
            ]
        );
    }

    #[test]
    fn colours_cycle_and_never_land_on_the_one_failures_use() {
        assert_eq!(color(0), color(STEP_COLORS.len()));
        for index in 0..STEP_COLORS.len() {
            assert_ne!(color(index), RED, "a step was coloured like a failure");
        }
    }

    #[test]
    fn colour_wraps_a_row_without_changing_what_it_says() {
        let line = status_line(&outcome("rust", Status::Done), 4, true);
        assert!(line.contains("\x1b["), "nothing was coloured");
        assert!(line.contains("rust"), "{line}");
    }

    #[test]
    fn padding_counts_characters_rather_than_bytes() {
        assert_eq!(pad("æøå", 5), "æøå  ");
        assert_eq!(pad("wider", 2), "wider");
    }
}
