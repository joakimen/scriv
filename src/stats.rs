//! What has been run, how often, and how long it took.
//!
//! I/O-free: [`crate::cmd::stats`] opens the log and appends to it. The only
//! clock here is the one measuring a run, and the counter the selectors add
//! their waiting to.
//!
//! Every run appends one line rather than rewriting a total, so two scriv
//! processes finishing at the same moment cannot lose each other's row, and a
//! run that is killed loses only itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::project::report::format_duration;
use crate::term;

/// The log every run appends to: `$XDG_DATA_HOME/scriv/stats`, falling back to
/// `~/.local/share`, as the spec puts machine-written data.
pub fn path(data_home: Option<&str>, home: &Path) -> PathBuf {
    let base = data_home
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    base.join("scriv").join("stats")
}

/// One run: when it finished, how long it took with the time spent waiting for
/// the person at the keyboard taken out, how much of that went on subprocesses,
/// and which command it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Unix seconds.
    pub at: i64,
    pub millis: u64,
    /// How much of `millis` was spent waiting for a subprocess to finish —
    /// `git`, `gh`, a build tool, an editor.
    pub child_millis: u64,
    /// The command as it is spelled, subcommands included: `repo sel`.
    pub command: String,
}

/// A record as its line of the log. Tab-separated with the command last, so a
/// name that ever grows a space in it still needs no quoting.
pub fn format(record: &Record) -> String {
    format!(
        "{}\t{}\t{}\t{}\n",
        record.at, record.millis, record.child_millis, record.command
    )
}

/// Every record in `text`.
///
/// A line that does not parse is skipped rather than failing the read: the log
/// is appended to by every run there has ever been, and one truncated row from
/// a machine that lost power is not a reason to refuse the rest.
///
/// Rows written before the subprocess column existed carry three fields and are
/// read as having spent none. The third field tells the two apart: a number
/// there is the column, anything else is the command.
pub fn parse(text: &str) -> Vec<Record> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Record> {
    let mut fields = line.splitn(4, '\t');
    let at = fields.next()?.trim().parse().ok()?;
    let millis = fields.next()?.trim().parse().ok()?;
    let third = fields.next()?.trim();
    let (child_millis, command) = match (third.parse(), fields.next()) {
        (Ok(child), Some(command)) => (child, command.trim()),
        _ => (0, third),
    };
    if command.is_empty() {
        return None;
    }
    Some(Record {
        at,
        millis,
        child_millis,
        command: command.to_string(),
    })
}

/// What one command adds up to over every run of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub calls: u64,
    /// Wall time over every run, which is what makes a command worth
    /// improving: a fast one run all day costs more than a slow one run twice.
    pub millis: u64,
    /// How much of [`Self::millis`] went on waiting for subprocesses.
    pub child_millis: u64,
}

impl Totals {
    /// What one run of it costs on average, and nothing when it has never run.
    pub fn average(&self) -> Option<Duration> {
        (self.calls > 0).then(|| Duration::from_millis(self.millis / self.calls))
    }

    /// What one run costs in scriv's own code, with the subprocesses it waited
    /// on taken out.
    ///
    /// A lower bound rather than an exact figure: two children waited on at
    /// once are each counted in full, so a command that fans out reads as doing
    /// less of its own work than it does.
    pub fn own_average(&self) -> Option<Duration> {
        (self.calls > 0).then(|| {
            Duration::from_millis(self.millis.saturating_sub(self.child_millis) / self.calls)
        })
    }

    fn add(&mut self, other: Totals) {
        self.calls += other.calls;
        self.millis += other.millis;
        self.child_millis += other.child_millis;
    }
}

/// What each command adds up to, by the name it was run under.
pub fn totals(records: &[Record]) -> BTreeMap<String, Totals> {
    let mut out: BTreeMap<String, Totals> = BTreeMap::new();
    for record in records {
        out.entry(record.command.clone()).or_default().add(Totals {
            calls: 1,
            millis: record.millis,
            child_millis: record.child_millis,
        });
    }
    out
}

// --- the tree ----------------------------------------------------------------

/// One command, and the commands under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub children: Vec<Node>,
}

/// The command tree, as clap knows it.
///
/// Hidden commands are left out, and so is clap's own `help`: it is not one of
/// scriv's commands, and the mirror of every other command it carries would
/// draw the whole tree twice.
pub fn tree(command: &clap::Command) -> Node {
    Node {
        name: command.get_name().to_string(),
        children: command
            .get_subcommands()
            .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
            .map(tree)
            .collect(),
    }
}

/// One line of the report: the branch drawn to its left, the command's own
/// name, and what it and everything under it add up to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// The box-drawing that connects this row to the ones above it.
    pub branch: String,
    pub name: String,
    /// The full command, as the log spells it.
    pub command: String,
    pub totals: Totals,
    /// Whether anything below this row is counted in its totals, which is what
    /// makes the number a sum rather than a count of its own runs.
    pub is_group: bool,
}

/// The tree as rows, in the order they are drawn, each carrying the totals of
/// the whole subtree below it.
///
/// The root is a row of its own: what the tool adds up to altogether.
pub fn rows(node: &Node, totals: &BTreeMap<String, Totals>) -> Vec<TreeRow> {
    let mut out = Vec::new();
    walk(node, "", "", "", totals, &mut out);
    out
}

/// Depth-first, building each row's branch from its parent's — `│  ` where the
/// parent still has siblings below it, three spaces where it does not — and
/// summing each subtree into its parent on the way back out.
///
/// `command` is what the log spells this row, which is empty at the root: a run
/// is recorded as `repo sel`, not as `scriv repo sel`.
fn walk(
    node: &Node,
    branch: &str,
    below: &str,
    command: &str,
    totals: &BTreeMap<String, Totals>,
    out: &mut Vec<TreeRow>,
) -> Totals {
    let index = out.len();
    let mut own = totals.get(command).copied().unwrap_or_default();
    out.push(TreeRow {
        branch: branch.to_string(),
        name: node.name.clone(),
        command: command.to_string(),
        totals: own,
        is_group: !node.children.is_empty(),
    });

    let last = node.children.len().saturating_sub(1);
    for (i, child) in node.children.iter().enumerate() {
        let (glyph, gap) = if i == last {
            ("└─ ", "   ")
        } else {
            ("├─ ", "│  ")
        };
        let path = if command.is_empty() {
            child.name.clone()
        } else {
            format!("{command} {}", child.name)
        };
        own.add(walk(
            child,
            &format!("{below}{glyph}"),
            &format!("{below}{gap}"),
            &path,
            totals,
            out,
        ));
    }

    out[index].totals = own;
    own
}

/// The columns, as `stats show` prints them: the tree, how often each command
/// has been run, what one run of it costs, and how much of that is scriv's own
/// work rather than a subprocess it waited on.
pub fn render(rows: &[TreeRow], color: bool) -> Vec<String> {
    let duration = |value: Option<Duration>| {
        value
            .map(format_duration)
            .unwrap_or_else(|| "-".to_string())
    };
    let cell = |row: &TreeRow| {
        (
            match row.totals.calls {
                0 => "-".to_string(),
                calls => calls.to_string(),
            },
            duration(row.totals.average()),
            duration(row.totals.own_average()),
        )
    };

    let name_width = rows
        .iter()
        .map(|row| row.branch.chars().count() + row.name.chars().count())
        .max()
        .unwrap_or(0)
        .max(HEADINGS.0.len());
    let calls_width = rows
        .iter()
        .map(|row| cell(row).0.chars().count())
        .max()
        .unwrap_or(0)
        .max(HEADINGS.1.len());
    let average_width = rows
        .iter()
        .map(|row| cell(row).1.chars().count())
        .max()
        .unwrap_or(0)
        .max(HEADINGS.2.len());

    let mut out = vec![format!(
        "{}  {}  {}  {}",
        term::bold(&pad(HEADINGS.0, name_width), color),
        term::bold(&right(HEADINGS.1, calls_width), color),
        term::bold(&right(HEADINGS.2, average_width), color),
        term::bold(HEADINGS.3, color),
    )];
    out.extend(rows.iter().map(|row| {
        let (calls, average, own) = cell(row);
        let drawn = row.branch.chars().count() + row.name.chars().count();
        format!(
            "{}{}{}  {}  {}  {}",
            term::paint(&row.branch, term::SECONDARY, color),
            // A command nobody has run is secondary text: the point of the
            // report is the ones that are.
            match row.totals.calls {
                0 => term::paint(&row.name, term::SECONDARY, color),
                _ => term::bold(&row.name, color),
            },
            " ".repeat(name_width.saturating_sub(drawn)),
            term::paint(&right(&calls, calls_width), term::SECONDARY, color),
            term::paint(&right(&average, average_width), term::SECONDARY, color),
            term::paint(&own, term::SECONDARY, color),
        )
        .trim_end()
        .to_string()
    }));
    out
}

/// What the four columns are called. `own` is the average with the time spent
/// waiting on subprocesses taken out.
const HEADINGS: (&str, &str, &str, &str) = ("command", "runs", "average", "own");

fn pad(text: &str, width: usize) -> String {
    format!(
        "{text}{}",
        " ".repeat(width.saturating_sub(text.chars().count()))
    )
}

fn right(text: &str, width: usize) -> String {
    format!(
        "{}{text}",
        " ".repeat(width.saturating_sub(text.chars().count()))
    )
}

// --- the prompt ---------------------------------------------------------------

/// How many commands the hand-off names. Enough to see a pattern, few enough
/// that the ones at the top are the ones being asked about.
const IMPROVE_ROWS: usize = 10;

/// The commands worth improving, in the order they are worth it: total time
/// spent, which is the one number that has both how often a command is run and
/// what each run costs in it.
pub fn by_value(rows: &[TreeRow]) -> Vec<&TreeRow> {
    let mut leaves: Vec<&TreeRow> = rows
        .iter()
        .filter(|row| !row.is_group && row.totals.calls > 0)
        .collect();
    leaves.sort_by(|a, b| {
        b.totals
            .millis
            .cmp(&a.totals.millis)
            .then_with(|| b.totals.calls.cmp(&a.totals.calls))
            .then_with(|| a.command.cmp(&b.command))
    });
    leaves
}

/// What `stats improve` asks Claude Code to do, with the table it should read
/// it against.
pub fn improve_prompt(rows: &[TreeRow]) -> String {
    let mut prompt = String::from(
        "These are scriv's own usage statistics, gathered by `scriv stats`. \
         Each row is a command, how many times it has been run, what one run \
         costs on average, how much of that average is scriv's own work, and \
         what the command has cost in total.\n\n\
         | command | runs | average | own | total |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for row in by_value(rows).into_iter().take(IMPROVE_ROWS) {
        let duration =
            |value: Option<Duration>| value.map(format_duration).unwrap_or_else(|| "-".into());
        prompt.push_str(&format!(
            "| `scriv {}` | {} | {} | {} | {} |\n",
            row.command,
            row.totals.calls,
            duration(row.totals.average()),
            duration(row.totals.own_average()),
            format_duration(Duration::from_millis(row.totals.millis)),
        ));
    }
    prompt.push_str(
        "\nThe rows are ordered by total time spent, which is where making \
         scriv faster is worth the most. Time spent waiting for the user in a \
         selector is already excluded from these numbers. `own` takes out the \
         subprocesses a run waited on as well — `git`, `gh`, a build tool, an \
         editor — so it is what is left for scriv's own code to answer for.\n\n\
         A row whose `own` is far below its average spends its time in \
         something scriv shells out to. That is not a reason to pass it over: \
         which subprocess gets called, and with which arguments, is scriv's \
         choice, and a cheaper one that reaches the same answer is often \
         there. Beware the average as well — a handful of slow runs among many \
         fast ones raises it without there being anything slow about the \
         command, so look at the log itself before concluding where the time \
         went.\n\n\
         Take the highest-value row you can actually improve, work out where \
         its time goes, and implement the improvement. Measure before and \
         after and say what the numbers were. Follow CLAUDE.md in this \
         repository, including how work is branched and shipped.",
    );
    prompt
}

/// The one command whose run is not recorded.
const RESET: &str = "stats reset";

/// Whether a run is one to record.
///
/// Every run but the one that forgets every run: recording that would leave
/// the log holding the command that had just emptied it, and the count it
/// reports the next time would be of itself.
pub fn records(command: &str) -> bool {
    command != RESET
}

// --- the clock ----------------------------------------------------------------

/// Nanoseconds this process has spent waiting for the person at the keyboard.
///
/// A process-wide counter rather than something threaded through every call:
/// what it measures is a property of the process, the selector that adds to it
/// is three call levels below the command being timed, and nothing decides
/// anything by it.
static WAITED: AtomicU64 = AtomicU64::new(0);

/// Time spent in front of a person, counted from when this is bound until it is
/// dropped. Bind it — `let _waiting = stats::interacting()`.
#[must_use]
pub struct Interaction(Instant);

/// Start counting time that belongs to the user rather than to scriv.
pub fn interacting() -> Interaction {
    Interaction(Instant::now())
}

impl Drop for Interaction {
    fn drop(&mut self) {
        let waited = u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        WAITED.fetch_add(waited, Ordering::Relaxed);
    }
}

/// How long this process has waited for the person running it.
pub fn waited() -> Duration {
    Duration::from_nanos(WAITED.load(Ordering::Relaxed))
}

/// Nanoseconds this process has spent waiting for subprocesses to finish.
static IN_CHILD: AtomicU64 = AtomicU64::new(0);

/// Time spent waiting for a spawned process, counted from when this is bound
/// until it is dropped. Bind it — `let _child = stats::in_child()`.
///
/// Bound around the wait, not around the spawn, so a child that scriv starts
/// and reads from while it runs is counted for as long as scriv is held up by
/// it. A counter rather than a value threaded through the call graph, for the
/// reason [`Interaction`] is one, and independent of it: a child spawned while
/// a selector is open is counted by both.
#[must_use]
pub struct Child(Instant);

/// Start counting time that belongs to a subprocess rather than to scriv.
pub fn in_child() -> Child {
    Child(Instant::now())
}

impl Drop for Child {
    fn drop(&mut self) {
        let spent = u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX);
        IN_CHILD.fetch_add(spent, Ordering::Relaxed);
    }
}

/// How long this process has spent waiting for the ones it spawned.
pub fn child_time() -> Duration {
    Duration::from_nanos(IN_CHILD.load(Ordering::Relaxed))
}

/// What the run itself took: the wall clock less the time the user spent
/// deciding. A selector left open over lunch is not a slow command.
pub fn ran_for(total: Duration, waited: Duration) -> Duration {
    total.saturating_sub(waited)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run that spent `child_millis` of its `millis` waiting on a subprocess.
    fn child_record(command: &str, millis: u64, child_millis: u64) -> Record {
        Record {
            child_millis,
            ..record(command, millis)
        }
    }

    fn record(command: &str, millis: u64) -> Record {
        Record {
            at: 1_700_000_000,
            millis,
            child_millis: 0,
            command: command.to_string(),
        }
    }

    #[test]
    fn a_record_survives_the_round_trip_through_a_line() {
        let mut written_record = record("repo sel", 842);
        written_record.child_millis = 800;
        let written = format(&written_record);
        assert_eq!(written, "1700000000\t842\t800\trepo sel\n");
        assert_eq!(parse(&written), vec![written_record]);
    }

    /// The log outlives the format: every run ever recorded is in it, and the
    /// rows written before subprocesses were counted still have to read back.
    #[test]
    fn a_row_from_before_the_subprocess_column_reads_as_having_spent_none() {
        assert_eq!(
            parse("1700000000\t842\trepo sel\n"),
            vec![record("repo sel", 842)]
        );
    }

    /// A command name is the one field that could be mistaken for the column
    /// ahead of it, and a command named for a number is still a command.
    #[test]
    fn a_command_that_looks_like_a_number_is_still_read_as_the_command() {
        let records = parse("1700000000\t842\t404\n");
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].command, "404");
        assert_eq!(records[0].child_millis, 0);
    }

    /// The log is every run there has ever been, and a machine that lost power
    /// mid-append is not a reason to refuse the rest of it.
    #[test]
    fn a_line_that_makes_no_sense_is_skipped_rather_than_failing_the_read() {
        let records = parse(
            "1700000000\t10\trepo ls\n\
             nonsense\n\
             \n\
             1700000000\tnot-a-number\tpr ls\n\
             1700000001\t20\t\n\
             1700000002\t30\tconfig print\n\
             1700000",
        );
        assert_eq!(
            records
                .iter()
                .map(|r| r.command.as_str())
                .collect::<Vec<_>>(),
            ["repo ls", "config print"]
        );
    }

    #[test]
    fn totals_count_the_runs_and_add_up_what_they_cost() {
        let totals = totals(&[
            record("repo sel", 100),
            record("repo sel", 300),
            record("pr ls", 50),
        ]);
        assert_eq!(
            totals["repo sel"],
            Totals {
                calls: 2,
                millis: 400,
                child_millis: 0
            }
        );
        assert_eq!(
            totals["repo sel"].average(),
            Some(Duration::from_millis(200))
        );
        assert_eq!(totals["pr ls"].calls, 1);
        assert_eq!(Totals::default().average(), None, "no runs, no average");
    }

    /// A selector left open over lunch is not a slow command.
    #[test]
    fn what_the_user_spent_deciding_is_not_what_the_command_took() {
        assert_eq!(
            ran_for(Duration::from_secs(30), Duration::from_secs(29)),
            Duration::from_secs(1)
        );
        // A clock that disagrees with itself is a zero, not a panic.
        assert_eq!(
            ran_for(Duration::from_secs(1), Duration::from_secs(2)),
            Duration::ZERO
        );
    }

    /// A reset that recorded itself would empty the log and immediately put a
    /// row back in it — and which of the two won was down to whether the log
    /// had been opened yet.
    #[test]
    fn the_run_that_forgets_every_run_does_not_record_itself() {
        assert!(!records("stats reset"));
        assert!(records("stats show"));
        assert!(records("repo sel"));
    }

    #[test]
    fn the_log_is_under_the_data_directory_the_environment_names() {
        assert_eq!(
            path(Some("/data"), Path::new("/home/me")),
            PathBuf::from("/data/scriv/stats")
        );
        // Unset, and empty, fall back to where the spec puts it.
        for unset in [None, Some(""), Some("  ")] {
            assert_eq!(
                path(unset, Path::new("/home/me")),
                PathBuf::from("/home/me/.local/share/scriv/stats")
            );
        }
    }

    fn cli() -> clap::Command {
        clap::Command::new("scriv")
            .subcommand(
                clap::Command::new("repo")
                    .subcommand(clap::Command::new("ls"))
                    .subcommand(clap::Command::new("sel")),
            )
            .subcommand(clap::Command::new("edit"))
            .subcommand(clap::Command::new("secret").hide(true))
    }

    /// The tree is every command there is, which is what makes the report say
    /// what you are not using as well as what you are.
    #[test]
    fn the_tree_is_claps_own_without_what_it_hides() {
        let tree = tree(&cli());
        assert_eq!(tree.name, "scriv");
        assert_eq!(
            tree.children
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["repo", "edit"],
            "a hidden command is not one to report on"
        );
        assert_eq!(tree.children[0].children.len(), 2);
    }

    #[test]
    fn a_row_is_named_as_the_log_spells_it_and_the_root_is_not_part_of_it() {
        let rows = rows(&tree(&cli()), &BTreeMap::new());
        let commands: Vec<&str> = rows.iter().map(|r| r.command.as_str()).collect();
        assert_eq!(commands, ["", "repo", "repo ls", "repo sel", "edit"]);

        let drawn: Vec<String> = rows
            .iter()
            .map(|r| format!("{}{}", r.branch, r.name))
            .collect();
        assert_eq!(
            drawn,
            ["scriv", "├─ repo", "│  ├─ ls", "│  └─ sel", "└─ edit"]
        );
    }

    /// A group is what the commands under it come to, so the tree can be read
    /// from the top: the root is the whole tool.
    #[test]
    fn a_group_adds_up_what_is_under_it() {
        let totals = totals(&[
            record("repo ls", 100),
            record("repo sel", 300),
            record("repo sel", 500),
            record("edit", 40),
        ]);
        let rows = rows(&tree(&cli()), &totals);
        let of = |command: &str| {
            rows.iter()
                .find(|r| r.command == command)
                .unwrap_or_else(|| panic!("no {command} row"))
                .totals
        };

        assert_eq!(of("repo sel").calls, 2);
        assert_eq!(
            of("repo"),
            Totals {
                calls: 3,
                millis: 900,
                child_millis: 0
            },
            "a group is its children"
        );
        assert_eq!(
            of(""),
            Totals {
                calls: 4,
                millis: 940,
                child_millis: 0
            },
            "the root is everything"
        );
    }

    /// The whole point of the column: a command that is slow because of what
    /// it shells out to has to be told apart from one that is slow itself.
    #[test]
    fn what_a_run_spent_in_a_subprocess_comes_off_its_own_time() {
        let totals = totals(&[
            child_record("repo open", 600, 540),
            child_record("repo open", 400, 360),
        ]);
        let repo_open = totals["repo open"];
        assert_eq!(repo_open.average(), Some(Duration::from_millis(500)));
        assert_eq!(repo_open.own_average(), Some(Duration::from_millis(50)));
    }

    /// Two children waited on at once are each counted in full, so the tally
    /// can exceed the run. The floor is zero rather than an underflow.
    #[test]
    fn a_run_that_waited_on_more_than_it_lasted_owns_none_of_it() {
        let totals = totals(&[child_record("pr ls", 100, 180)]);
        assert_eq!(totals["pr ls"].own_average(), Some(Duration::ZERO));
    }

    #[test]
    fn a_command_nobody_has_run_says_so_rather_than_showing_a_zero() {
        let lines = render(&rows(&tree(&cli()), &BTreeMap::new()), false);
        assert!(lines[0].starts_with("command"), "{:?}", lines[0]);
        assert!(lines[1].contains(" - "), "{:?}", lines[1]);
        for line in &lines {
            assert!(!line.contains('\x1b'), "colour leaked into a plain report");
            assert!(!line.ends_with(' '), "trailing padding: {line:?}");
        }
    }

    #[test]
    fn the_columns_line_up() {
        let totals = totals(&[record("repo sel", 1234), record("edit", 5)]);
        let lines = render(&rows(&tree(&cli()), &totals), false);
        // Where the last field starts, counted in characters: the branch is
        // drawn in box characters three bytes wide, so byte offsets would say
        // rows that line up on screen do not.
        let column = |line: &str| {
            line.rsplit_once(' ')
                .map(|(head, _)| head.chars().count() + 1)
        };
        let first = column(&lines[0]);
        assert!(first.is_some());
        for line in &lines {
            assert_eq!(column(line), first, "{line}");
        }
    }

    /// Total time spent is the one number with both how often a command runs
    /// and what each run costs in it.
    #[test]
    fn the_commands_worth_improving_are_ordered_by_what_they_have_cost() {
        let totals = totals(&[
            // Slow, but run once.
            record("repo ls", 900),
            // Quick, but run all day.
            record("repo sel", 200),
            record("repo sel", 200),
            record("repo sel", 200),
            record("repo sel", 200),
            record("repo sel", 200),
            // Slow, and none of it scriv's: the editor held the terminal.
            child_record("edit", 3_000, 2_990),
        ]);
        let rows = rows(&tree(&cli()), &totals);

        let ranked: Vec<&str> = by_value(&rows).iter().map(|r| r.command.as_str()).collect();
        assert_eq!(ranked, ["edit", "repo sel", "repo ls"]);
        // Groups are not rows to improve: `repo` is not a command anyone runs.
        assert!(!ranked.contains(&"repo"), "{ranked:?}");

        let prompt = improve_prompt(&rows);
        assert!(
            prompt.contains("`scriv repo sel` | 5 | 200ms | 200ms | 1.0s"),
            "{prompt}"
        );
        // The row that only looks expensive says so in the column beside it.
        assert!(
            prompt.contains("`scriv edit` | 1 | 3.0s | 10ms |"),
            "{prompt}"
        );
        assert!(prompt.contains("CLAUDE.md"), "{prompt}");
    }
}
