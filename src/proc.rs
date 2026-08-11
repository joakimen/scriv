//! Running processes: parsing `ps` output, ordering it, and rendering rows.
//!
//! No I/O — [`crate::cmd::proc`] runs `ps` and hands the text here. `ps` is
//! spawned rather than the process table read directly because every supported
//! platform ships it.

use std::collections::HashSet;

use anyhow::{Result, bail};

use crate::term;

/// The `ps` field list scriv reads, in the order [`parse`] expects. The
/// trailing `=` suppresses each header, so every line is a process; `args`
/// comes last because it is the only field that can contain spaces.
pub const PS_ARGS: [&str; 3] = ["-axo", "user=,pid=,ppid=,pcpu=,pmem=,etime=,args=", "-ww"];

/// How many fixed-width fields precede the command in a `ps` line.
const FIELDS: usize = 6;

/// One running process, as `ps` reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Process {
    pub pid: i32,
    pub ppid: i32,
    pub user: String,
    pub cpu: f32,
    pub mem: f32,
    /// Elapsed running time, in `ps`'s own `[[dd-]hh:]mm:ss` form.
    pub elapsed: String,
    /// The full command line, arguments included.
    pub command: String,
}

impl Process {
    /// The process name alone: the last path component of argv[0].
    pub fn name(&self) -> &str {
        let argv0 = self.command.split_whitespace().next().unwrap_or("");
        argv0.rsplit('/').next().unwrap_or(argv0)
    }
}

/// Parse the output of `ps` with [`PS_ARGS`]. Unparseable lines are skipped
/// rather than failing the listing.
///
/// Every process chooses its own argv, so the text is made safe here — see
/// [`term::one_row`] — rather than at each place that draws it.
pub fn parse(text: &str) -> Vec<Process> {
    text.lines().filter_map(parse_line).collect()
}

fn parse_line(line: &str) -> Option<Process> {
    let mut rest = line.trim_start();
    let mut fields = [""; FIELDS];
    for field in &mut fields {
        let end = rest.find(char::is_whitespace)?;
        *field = &rest[..end];
        rest = rest[end..].trim_start();
    }
    let command = rest.trim_end();
    if command.is_empty() {
        return None;
    }
    Some(Process {
        user: term::one_row(fields[0]),
        pid: fields[1].parse().ok()?,
        ppid: fields[2].parse().ok()?,
        cpu: fields[3].parse().ok()?,
        mem: fields[4].parse().ok()?,
        elapsed: term::one_row(fields[5]),
        command: term::one_row(command),
    })
}

/// `pid` and every process above it in the parent chain. Walked defensively: a
/// table read while processes were exiting can contain a cycle.
pub fn ancestry(processes: &[Process], pid: i32) -> HashSet<i32> {
    let mut chain = HashSet::new();
    let mut current = pid;
    while chain.insert(current) {
        let Some(parent) = processes.iter().find(|p| p.pid == current) else {
            break;
        };
        if parent.ppid <= 0 {
            break;
        }
        current = parent.ppid;
    }
    chain
}

/// Why a pid given on the command line will not be signalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// `0` or negative. To `kill(2)` these are process *groups*, not
    /// processes: `scriv proc kill -- -1` would end the login session.
    NotAProcess,
    /// scriv itself, or something that spawned it — the chain that runs up
    /// through the shell to the terminal emulator.
    OwnAncestry,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Self::NotAProcess => {
                "not a process id — `0` means this process group and a negative \
                 number means a whole process group; use `kill` directly if that \
                 is what you meant"
            }
            Self::OwnAncestry => "scriv itself or a process that spawned it",
        }
    }
}

/// Check pids given as arguments against the rules [`selectable`] enforces by
/// never offering the row in the first place.
pub fn refuse(processes: &[Process], self_pid: i32, pids: &[i32]) -> Vec<(i32, Refusal)> {
    let own = ancestry(processes, self_pid);
    pids.iter()
        .filter_map(|&pid| {
            if pid <= 0 {
                Some((pid, Refusal::NotAProcess))
            } else if own.contains(&pid) {
                Some((pid, Refusal::OwnAncestry))
            } else {
                None
            }
        })
        .collect()
}

/// The processes worth offering, busiest first. scriv's own process and
/// everything that spawned it are dropped: `-9` on any of that chain takes the
/// session with it.
pub fn selectable(processes: &[Process], self_pid: i32) -> Vec<Process> {
    let own = ancestry(processes, self_pid);
    let mut visible: Vec<Process> = processes
        .iter()
        .filter(|p| !own.contains(&p.pid))
        .cloned()
        .collect();
    // Ties break by pid so the order is stable between runs.
    visible.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.pid.cmp(&b.pid))
    });
    visible
}

/// A plain listing row: the pid and the command, one space apart, so
/// `scriv proc ls | grep node | cut -d' ' -f1` reaches the pid.
pub fn plain_row(p: &Process) -> String {
    format!("{} {}", p.pid, p.command)
}

/// The width the pid column is padded to, so the columns after it line up.
const PID_WIDTH: usize = 7;

/// The widest the user column is allowed to get: enough for a login name and
/// for the system accounts that recur, but not for the longest daemon on the
/// machine.
const MAX_USER_WIDTH: usize = 14;

/// How wide the user column has to be for `procs` to line up: measured, and
/// capped at [`MAX_USER_WIDTH`]. Past the cap a name is printed in full and
/// pushes its own row out, rather than being cut into an ambiguous one.
pub fn user_width(procs: &[Process]) -> usize {
    procs
        .iter()
        .map(|p| p.user.chars().count())
        .max()
        .unwrap_or(0)
        .min(MAX_USER_WIDTH)
}

/// The row `--status` prints and the selector shows: pid, user, cpu, elapsed
/// running time, then the command. `user_width` comes from [`user_width`] over
/// the whole listing. Everything before the command is drawn grey.
pub fn status_row(p: &Process, user_width: usize, color: bool) -> String {
    let head = format!(
        "{:>PID_WIDTH$} {:<user_width$} {:>5.1} {:>11}",
        p.pid, p.user, p.cpu, p.elapsed
    );
    format!(
        "{} {}",
        crate::term::paint(&head, CONTEXT_COLOR, color),
        p.command
    )
}

/// Grey, the terminal's own, for the columns before the command.
const CONTEXT_COLOR: u8 = 8;

/// The preview pane for a highlighted row, built from the single `ps` call the
/// listing already made rather than a command of its own.
pub fn preview(p: &Process) -> String {
    format!(
        "{}\n\n  pid      {}\n  ppid     {}\n  user     {}\n  cpu      {:.1}%\n  \
         mem      {:.1}%\n  elapsed  {}\n\n{}\n",
        p.name(),
        p.pid,
        p.ppid,
        p.user,
        p.cpu,
        p.mem,
        p.elapsed,
        p.command,
    )
}

/// A signal `scriv proc kill` can send, named as `kill` names it. A closed set,
/// so an unusable signal is rejected before the selector opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signal(&'static str);

/// scriv builds for Apple Silicon and refuses anywhere else, and this table is
/// why the refusal is a compile error rather than a line in the README. Only
/// the first five signals below are numbered by POSIX. The rest are Darwin's,
/// and Linux disagrees in the worst possible way: 19 is `CONT` here and `STOP`
/// there, so the same table on the wrong platform does not fail — it resumes
/// the process the user meant to suspend.
#[cfg(not(target_vendor = "apple"))]
compile_error!(
    "scriv supports macOS on Apple Silicon only: its signal numbers are Darwin's, \
     and elsewhere they name a different signal than the one asked for"
);

/// The signals worth offering, by the names `kill -l` prints: the ones that end
/// or suspend a process. Anything else is a job for `kill`.
const SIGNALS: [(&str, i32); 9] = [
    ("HUP", 1),
    ("INT", 2),
    ("QUIT", 3),
    ("KILL", 9),
    ("TERM", 15),
    ("USR1", 30),
    ("USR2", 31),
    ("STOP", 17),
    ("CONT", 19),
];

impl Signal {
    /// The default: ask the process to stop and let it clean up. `--force` is
    /// there for the uncatchable `KILL`.
    pub const TERM: Self = Self("TERM");
    /// Unblockable, for when `TERM` has been ignored.
    pub const KILL: Self = Self("KILL");

    /// Parse a signal as written on the command line: `TERM`, `SIGTERM`,
    /// `term` and `15` are all the same signal.
    pub fn parse(input: &str) -> Result<Self> {
        let name = input.trim().to_uppercase();
        let name = name.strip_prefix("SIG").unwrap_or(&name);
        if let Ok(number) = name.parse::<i32>() {
            return match SIGNALS.iter().find(|(_, n)| *n == number) {
                Some((name, _)) => Ok(Self(name)),
                None => bail!("unknown signal `{input}` — {}", Self::known()),
            };
        }
        match SIGNALS.iter().find(|(n, _)| *n == name) {
            Some((name, _)) => Ok(Self(name)),
            None => bail!("unknown signal `{input}` — {}", Self::known()),
        }
    }

    /// The name to hand `kill`, and to print in the report afterwards.
    pub fn name(self) -> &'static str {
        self.0
    }

    fn known() -> String {
        let names: Vec<&str> = SIGNALS.iter().map(|(n, _)| *n).collect();
        format!("known signals are {}", names.join(", "))
    }
}

impl std::fmt::Display for Signal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `ps -axo user=,pid=,ppid=,pcpu=,pmem=,etime=,args= -ww` output:
    /// right-aligned numeric columns, a command that contains spaces, and one
    /// that contains no path at all.
    const SAMPLE: &str = "\
joakim            501     1   12.3  1.5    03:12:44 /usr/local/bin/node server.js --port 3000
root              1       0    0.0  0.1 12-04:11:02 /sbin/launchd
joakim          70123 70100    0.0  0.4       01:20 -fish
";

    fn sample() -> Vec<Process> {
        parse(SAMPLE)
    }

    #[test]
    fn parse_reads_every_column_and_keeps_the_command_whole() {
        let procs = sample();
        assert_eq!(procs.len(), 3);
        assert_eq!(
            procs[0],
            Process {
                pid: 501,
                ppid: 1,
                user: "joakim".into(),
                cpu: 12.3,
                mem: 1.5,
                elapsed: "03:12:44".into(),
                command: "/usr/local/bin/node server.js --port 3000".into(),
            }
        );
    }

    #[test]
    fn a_command_keeps_its_arguments() {
        assert_eq!(
            sample()[0].command,
            "/usr/local/bin/node server.js --port 3000"
        );
    }

    #[test]
    fn unreadable_lines_are_skipped_rather_than_failing() {
        let text = format!("{SAMPLE}garbage\n  \n99999 1\n");
        assert_eq!(parse(&text).len(), 3);
    }

    #[test]
    fn name_is_the_last_component_of_argv0() {
        let procs = sample();
        assert_eq!(procs[0].name(), "node");
        assert_eq!(procs[1].name(), "launchd");
        assert_eq!(procs[2].name(), "-fish");
    }

    #[test]
    fn a_command_line_cannot_carry_an_escape_out_of_ps() {
        let procs = parse("joakim 501 1 0.0 0.1 01:00 /bin/evil \x1b[2K\x1b[1;32mhidden\n");
        assert_eq!(procs.len(), 1);
        assert!(!procs[0].command.contains('\x1b'), "{:?}", procs[0].command);
        assert_eq!(procs[0].command, "/bin/evil [2K[1;32mhidden");
    }

    #[test]
    fn ancestry_walks_from_a_pid_to_the_top() {
        let procs = sample();
        let chain = ancestry(&procs, 70123);
        assert!(chain.contains(&70123), "the process itself");
        assert!(
            chain.contains(&70100),
            "its parent, even though ps did not list it"
        );
    }

    #[test]
    fn a_pid_that_is_really_a_process_group_is_refused() {
        let procs = sample();
        for pid in [0, -1, -70123] {
            let refused = refuse(&procs, 501, &[pid]);
            assert_eq!(
                refused,
                vec![(pid, Refusal::NotAProcess)],
                "{pid} reached kill"
            );
        }
    }

    #[test]
    fn an_ancestor_named_as_an_argument_is_refused_too() {
        let procs = sample();
        // 70123 is `-fish`, whose parent 70100 stands in for the terminal.
        assert_eq!(
            refuse(&procs, 70123, &[70123]),
            vec![(70123, Refusal::OwnAncestry)]
        );
        assert_eq!(
            refuse(&procs, 70123, &[70100]),
            vec![(70100, Refusal::OwnAncestry)]
        );
    }

    #[test]
    fn an_unrelated_pid_passes() {
        assert!(refuse(&sample(), 70123, &[501, 1]).is_empty());
    }

    #[test]
    fn ancestry_terminates_on_a_cycle() {
        let procs = vec![
            Process {
                pid: 1,
                ppid: 2,
                user: "root".into(),
                cpu: 0.0,
                mem: 0.0,
                elapsed: "01:00".into(),
                command: "a".into(),
            },
            Process {
                pid: 2,
                ppid: 1,
                user: "root".into(),
                cpu: 0.0,
                mem: 0.0,
                elapsed: "01:00".into(),
                command: "b".into(),
            },
        ];
        assert_eq!(ancestry(&procs, 1), HashSet::from([1, 2]));
    }

    #[test]
    fn scriv_and_its_ancestors_are_not_offered() {
        let procs = sample();
        let pids: Vec<i32> = selectable(&procs, 70123).iter().map(|p| p.pid).collect();
        assert!(!pids.contains(&70123), "offered its own process");
        assert!(!pids.contains(&70100), "offered the shell that invoked it");
        assert!(pids.contains(&501), "dropped an unrelated process");
    }

    #[test]
    fn selectable_puts_the_busiest_first() {
        let cpus: Vec<f32> = selectable(&sample(), 0).iter().map(|p| p.cpu).collect();
        assert_eq!(cpus, vec![12.3, 0.0, 0.0]);
    }

    #[test]
    fn a_plain_row_leads_with_the_pid() {
        assert_eq!(
            plain_row(&sample()[0]),
            "501 /usr/local/bin/node server.js --port 3000"
        );
    }

    #[test]
    fn a_status_row_lines_its_columns_up() {
        let procs = sample();
        let width = user_width(&procs);
        let starts: Vec<usize> = procs
            .iter()
            .map(|p| {
                let row = status_row(p, width, false);
                row.len() - p.command.len()
            })
            .collect();
        assert!(
            starts.windows(2).all(|w| w[0] == w[1]),
            "commands started at {starts:?}"
        );
    }

    #[test]
    fn the_user_column_is_as_wide_as_the_longest_name() {
        let mut procs = sample();
        assert_eq!(user_width(&procs), "joakim".len());
        procs[1].user = "_windowserver".into();
        assert_eq!(user_width(&procs), "_windowserver".len());

        let width = user_width(&procs);
        let starts: Vec<usize> = procs
            .iter()
            .map(|p| status_row(p, width, false).len() - p.command.len())
            .collect();
        assert!(
            starts.windows(2).all(|w| w[0] == w[1]),
            "a long username broke the alignment: {starts:?}"
        );
    }

    #[test]
    fn one_very_long_name_does_not_widen_every_row() {
        let mut procs = sample();
        procs[1].user = "_installcoordinationd".into();
        assert_eq!(user_width(&procs), MAX_USER_WIDTH);
        // Printed in full even so — a cut username identifies nothing.
        assert!(
            status_row(&procs[1], user_width(&procs), false).contains("_installcoordinationd"),
            "the name was truncated"
        );
    }

    #[test]
    fn a_preview_carries_the_whole_command_line() {
        let text = preview(&sample()[0]);
        assert!(text.contains("--port 3000"), "{text}");
        assert!(text.contains("ppid     1"), "{text}");
    }

    #[test]
    fn a_signal_is_read_by_name_or_number_in_any_case() {
        for input in ["KILL", "kill", "SIGKILL", "sigkill", "9", " 9 "] {
            assert_eq!(Signal::parse(input).unwrap(), Signal::KILL, "{input}");
        }
    }

    #[test]
    fn an_unusable_signal_is_refused_up_front() {
        for input in ["NOPE", "0", "64", ""] {
            let err = Signal::parse(input).unwrap_err().to_string();
            assert!(err.contains("known signals are"), "{input}: {err}");
        }
    }

    /// Darwin's numbers, which the compile guard above is what makes safe to
    /// write down flat. Getting these wrong is not a parse error: `-s 19` is
    /// `CONT` here and `STOP` on Linux, so a process meant to be suspended
    /// carries on instead.
    #[test]
    fn the_signal_numbers_are_darwins() {
        let number = |name: &str| SIGNALS.iter().find(|(n, _)| *n == name).unwrap().1;
        assert_eq!(
            (
                number("USR1"),
                number("USR2"),
                number("STOP"),
                number("CONT"),
            ),
            (30, 31, 17, 19),
        );
    }

    /// The POSIX-fixed five, which are the same wherever they are read.
    #[test]
    fn the_portable_signals_are_numbered_as_posix_says() {
        for (name, number) in [
            ("HUP", 1),
            ("INT", 2),
            ("QUIT", 3),
            ("KILL", 9),
            ("TERM", 15),
        ] {
            assert_eq!(Signal::parse(&number.to_string()).unwrap().name(), name);
        }
    }

    #[test]
    fn the_default_signal_can_be_caught() {
        assert_eq!(Signal::TERM.name(), "TERM");
        assert_ne!(Signal::TERM, Signal::KILL);
    }

    #[test]
    fn a_status_row_is_plain_without_colour() {
        let p = &sample()[0];
        assert!(!status_row(p, 10, false).contains('\x1b'));
        assert!(status_row(p, 10, true).contains('\x1b'));
    }
}
