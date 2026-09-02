//! `scriv project` — build the directory you are standing in, and install what
//! it needs, without knowing beforehand what it is written in.
//!
//! The whole group is ambient: it acts on `$PWD` rather than on a set scriv
//! keeps, so there is nothing to list or select. What it finds there comes from
//! [`crate::project`], which decides everything; this module reads the
//! directory, runs the commands, and prints what happened.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::project::detect::{MANIFESTS, MISE_CONFIGS};
use crate::project::report::{Outcome, Status};
use crate::project::{Scan, Step, Toolchain, build, deps as manifests, detect, install, report};
use crate::{Ctx, Reported, term};

/// For what scriv says about a command rather than what the command said.
const DIM: u8 = term::SECONDARY;

/// `scriv project deps` — install every detected toolchain's dependencies.
///
/// `dump` reads the manifests instead of running anything, and `dry_run` names
/// the commands the run would have been.
pub fn deps(ctx: &Ctx, dry_run: bool, dump: bool) -> Result<()> {
    let dir = Path::new(ctx.pwd_str());
    let scan = scan(dir)?;
    let detections = detect::detect(&scan);

    if detections.is_empty() {
        return unrecognised(ctx, dir);
    }
    if dump {
        let manifests = manifests::list(&detections, &scan);
        return print(&manifests::listing(&manifests, ctx.color()));
    }

    let plan = install::plan(&detections);
    if dry_run {
        return print(&report::plan_listing(
            &plan.steps().collect::<Vec<_>>(),
            ctx.color(),
        ));
    }

    run(ctx, plan, dir)
}

/// `scriv project build` — run whatever building this project means.
pub fn build(ctx: &Ctx, dry_run: bool) -> Result<()> {
    let dir = Path::new(ctx.pwd_str());
    let scan = scan(dir)?;
    let detections = detect::detect(&scan);

    let steps = match build::plan(&scan, &detections) {
        build::Build::Ambiguous(files) => bail!(
            "{} both build this project — run the one you meant",
            files.join(" and ")
        ),
        build::Build::Steps(steps) if steps.is_empty() => bail!(
            "nothing here builds: no task runner, and nothing detected that builds on its own"
        ),
        build::Build::Steps(steps) => under_mise(steps, &detections),
    };

    if dry_run {
        return print(&report::plan_listing(
            &steps.iter().collect::<Vec<_>>(),
            ctx.color(),
        ));
    }

    // Sequentially, with the terminal handed straight to each command: a build
    // is watched while it runs, and two of them writing at once is unreadable.
    // The first failure ends the run, since what follows would be built against
    // what did not.
    for step in &steps {
        eprintln!(
            "{}",
            term::paint(&format!("$ {}", step.command_line()), DIM, ctx.color())
        );
        inherit(step, dir)?;
    }
    Ok(())
}

/// Run every build step through `mise exec` when the project pins its tools
/// with mise and mise is installed — the same resolution an activated shell
/// would have done, for one that has not activated it.
fn under_mise(steps: Vec<Step>, detections: &[detect::Detection]) -> Vec<Step> {
    let pinned = detections
        .iter()
        .any(|detection| detection.toolchain == Toolchain::Mise);
    if !pinned || crate::cmd::config::on_path("mise").is_none() {
        return steps;
    }
    steps
        .into_iter()
        .map(crate::project::through_mise)
        .collect()
}

/// What is said in a directory holding nothing any toolchain recognises. Not a
/// failure: asking is how you find out.
fn unrecognised(ctx: &Ctx, dir: &Path) -> Result<()> {
    eprintln!(
        "{}",
        term::paint(
            &format!("nothing recognisable in {}", dir.display()),
            DIM,
            ctx.color()
        )
    );
    Ok(())
}

fn print(lines: &[String]) -> Result<()> {
    let mut out = term::Listing::stdout();
    for line in lines {
        if !out.line(line)? {
            return Ok(());
        }
    }
    out.finish()?;
    Ok(())
}

/// Run the plan: `mise install` to completion first, then the rest at once.
///
/// The concurrency buys the wall clock of the slowest install rather than the
/// sum of them all — every one of them waits on a network it does not share
/// with the others. It costs the live ordering of the status lines, which
/// arrive as each step finishes rather than in plan order, and the output of a
/// failed step being held back until it has one.
fn run(ctx: &Ctx, plan: install::Plan, dir: &Path) -> Result<()> {
    let started = Instant::now();
    let color = ctx.color();
    let streaming = ctx.log.verbose();
    let names: Vec<&str> = plan.steps().map(|step| step.name).collect();
    let width = names
        .iter()
        .map(|name| name.chars().count())
        .max()
        .unwrap_or(0);
    let prefixes = report::prefixes(&names, color);
    let sink = |index: usize| {
        if streaming {
            Sink::Stream(&prefixes[index])
        } else {
            Sink::Capture
        }
    };

    let mut outcomes = Vec::new();
    let mut pinned = false;
    if let Some(step) = &plan.mise {
        let outcome = step_of(step, dir, sink(0), width, color);
        // Only a mise that actually installed can resolve the rest: wrapping
        // them after a failed install replaces each tool's own error with
        // mise's.
        pinned = outcome.status == Status::Done;
        outcomes.push(outcome);
    }

    let offset = usize::from(plan.mise.is_some());
    let steps: Vec<Step> = plan
        .parallel
        .into_iter()
        .map(|step| {
            if pinned {
                crate::project::through_mise(step)
            } else {
                step
            }
        })
        .collect();

    outcomes.extend(std::thread::scope(|scope| {
        let handles: Vec<_> = steps
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let sink = sink(offset + index);
                scope.spawn(move || step_of(step, dir, sink, width, color))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("an install thread panicked"))
            .collect::<Vec<_>>()
    }));

    if !streaming {
        details(&outcomes);
    }
    eprintln!();
    for line in report::summary(&outcomes, started.elapsed(), color) {
        eprintln!("{line}");
    }

    if outcomes.iter().any(Outcome::failed) {
        // The failing command's own output has just been printed above it.
        return Err(Reported(1).into());
    }
    Ok(())
}

/// Where a step's output goes while it runs.
#[derive(Clone, Copy)]
enum Sink<'a> {
    /// Held back until the step ends, and shown only if it failed. Steps
    /// running at once cannot interleave what they never wrote.
    Capture,
    /// Written to stderr as it arrives, behind the step's coloured prefix.
    Stream(&'a str),
}

fn step_of(step: &Step, dir: &Path, sink: Sink<'_>, width: usize, color: bool) -> Outcome {
    if let Sink::Stream(prefix) = sink {
        eprintln!(
            "{prefix}{}",
            term::paint(&format!("$ {}", step.command_line()), DIM, color)
        );
    }

    let started = Instant::now();
    let finished = match sink {
        Sink::Capture => capture(step, dir),
        Sink::Stream(prefix) => stream(step, dir, prefix),
    };
    let outcome = outcome(step, started.elapsed(), finished);

    // A streamed step has already said everything it had to say; a captured
    // one has said nothing until now.
    if matches!(sink, Sink::Capture) {
        eprintln!("{}", report::status_line(&outcome, width, color));
    }
    outcome
}

/// A finished child process, before it is read as an outcome.
struct Finished {
    status: ExitStatus,
    output: String,
}

fn outcome(step: &Step, duration: Duration, finished: io::Result<Finished>) -> Outcome {
    let (status, output) = match finished {
        Ok(finished) if finished.status.success() => (Status::Done, finished.output),
        Ok(finished) => (
            Status::Failed {
                code: finished.status.code(),
            },
            finished.output,
        ),
        // A tool the project asks for and the machine does not have. Reported
        // rather than failed: it is a machine to install it on, not a run that
        // went wrong.
        Err(error) if error.kind() == io::ErrorKind::NotFound => (
            Status::Skipped {
                reason: format!("{} not found", step.program),
            },
            String::new(),
        ),
        Err(error) => (Status::Failed { code: None }, error.to_string()),
    };

    Outcome {
        name: step.name,
        status,
        output,
        duration,
    }
}

fn command(step: &Step, dir: &Path) -> Command {
    let mut command = Command::new(&step.program);
    command.args(&step.args).current_dir(dir);
    command
}

fn capture(step: &Step, dir: &Path) -> io::Result<Finished> {
    let result = command(step, dir).stdin(Stdio::null()).output()?;

    Ok(Finished {
        status: result.status,
        output: merge(&result.stdout, &result.stderr),
    })
}

/// Forward both of a child's streams to stderr while it runs. They are read on
/// separate threads so a child that fills one pipe does not block writing to
/// the other.
fn stream(step: &Step, dir: &Path, prefix: &str) -> io::Result<Finished> {
    let mut child = command(step, dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    std::thread::scope(|scope| {
        scope.spawn(|| forward(stdout, prefix));
        scope.spawn(|| forward(stderr, prefix));
    });

    Ok(Finished {
        status: child.wait()?,
        output: String::new(),
    })
}

/// Write each line of `source` to stderr behind `prefix`. One `eprintln!` per
/// line is what keeps two steps' lines from landing inside one another.
fn forward(source: impl Read, prefix: &str) {
    let mut reader = BufReader::new(source);
    let mut line = Vec::new();

    while let Ok(read) = reader.read_until(b'\n', &mut line) {
        if read == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        eprintln!("{prefix}{}", text.trim_end_matches(['\n', '\r']));
        line.clear();
    }
}

/// Join a command's streams into the one block shown when it fails, stdout
/// first, with the trailing blank lines taken off.
fn merge(stdout: &[u8], stderr: &[u8]) -> String {
    let mut merged = String::new();

    for stream in [stdout, stderr] {
        let text = String::from_utf8_lossy(stream);
        let text = text.trim_end();
        if text.is_empty() {
            continue;
        }
        if !merged.is_empty() {
            merged.push('\n');
        }
        merged.push_str(text);
    }

    merged
}

/// Print what each failed step had to say, under a heading naming it. Written
/// as the command wrote it — colour included — since it is the same output the
/// user would have seen had they run it themselves.
fn details(outcomes: &[Outcome]) {
    for outcome in outcomes
        .iter()
        .filter(|outcome| outcome.failed() && !outcome.output.is_empty())
    {
        eprintln!();
        eprintln!("── {} ──", outcome.name);
        eprintln!("{}", outcome.output);
    }
}

/// Run a build step with the terminal handed to it, and pass its status on.
fn inherit(step: &Step, dir: &Path) -> Result<()> {
    let status = command(step, dir)
        .status()
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => anyhow!("`{}` was not found on PATH", step.program),
            _ => anyhow::Error::new(error).context(format!("running {}", step.command_line())),
        })?;

    if !status.success() {
        return Err(Reported(status.code().unwrap_or(1)).into());
    }
    Ok(())
}

/// Collect what detection and the dependency listing read: every entry in the
/// project root, the nested places mise also keeps its config, and the text of
/// each manifest that is there. A manifest that cannot be read is treated as
/// absent — the toolchain is still detected by its name being on disk.
fn scan(dir: &Path) -> Result<Scan> {
    let mut paths = BTreeSet::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        if let Some(name) = entry?.file_name().to_str() {
            paths.insert(name.to_string());
        }
    }
    for config in MISE_CONFIGS.iter().filter(|path| path.contains('/')) {
        if dir.join(config).is_file() {
            paths.insert((*config).to_string());
        }
    }

    let modules: Vec<String> = paths
        .iter()
        .filter(|path| path.ends_with(".tf"))
        .cloned()
        .collect();
    let mut contents = BTreeMap::new();
    for name in MANIFESTS
        .iter()
        .map(|name| (*name).to_string())
        .chain(modules)
    {
        if !paths.contains(&name) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(dir.join(&name)) {
            contents.insert(name, text);
        }
    }

    Ok(Scan { paths, contents })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_puts_stdout_before_stderr() {
        assert_eq!(merge(b"out\n", b"err\n"), "out\nerr");
    }

    #[test]
    fn merging_drops_the_streams_that_said_nothing() {
        assert_eq!(merge(b"", b"err\n"), "err");
        assert_eq!(merge(b"out\n", b""), "out");
        assert_eq!(merge(b"", b""), "");
    }

    #[test]
    fn merging_keeps_invalid_utf8_readable() {
        assert_eq!(merge(&[0xff, b'a'], b""), "\u{fffd}a");
    }

    #[test]
    fn a_directory_is_scanned_for_the_manifests_that_are_in_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
        std::fs::write(dir.path().join("README.md"), "unread").unwrap();
        std::fs::create_dir_all(dir.path().join(".config/mise")).unwrap();
        std::fs::write(dir.path().join(".config/mise/config.toml"), "[tools]").unwrap();

        let scan = scan(dir.path()).unwrap();

        assert!(scan.has("Cargo.toml"));
        assert!(scan.has(".config/mise/config.toml"), "{:?}", scan.paths);
        assert_eq!(scan.text("Cargo.toml"), Some("[package]"));
        assert_eq!(scan.text("README.md"), None, "read a file it never needs");
    }

    #[test]
    fn every_terraform_module_in_the_root_is_read() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.tf"), "resource {}").unwrap();
        std::fs::write(dir.path().join("versions.tf"), "terraform {}").unwrap();

        assert_eq!(
            scan(dir.path()).unwrap().terraform(),
            "resource {}\nterraform {}"
        );
    }
}
