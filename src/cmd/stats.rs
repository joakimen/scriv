//! `scriv stats` — what has been run, how often, and how long it took.
//!
//! The log is opened on a thread of its own while the command runs, so nothing
//! about recording a run is in front of the user: by the time there is a record
//! to write, the file is already open and the write is one append.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;

use anyhow::{Result, bail};

use crate::{Ctx, stats, term};

/// The open log, and the thread that owns it.
///
/// Failure is silent throughout: a tool that will not run because it cannot
/// count its own runs is worse than one that misses a row.
pub struct Recorder {
    /// `None` when there was nowhere to write, which is the whole of what a
    /// failure means here.
    tx: Option<Sender<stats::Record>>,
    thread: Option<JoinHandle<()>>,
}

impl Recorder {
    /// Open the log in the background and hand back something to finish with.
    pub fn start(path: PathBuf) -> Self {
        let (tx, rx) = channel::<stats::Record>();
        let thread = std::thread::spawn(move || {
            let file = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                .ok()
                .and_then(|()| {
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .ok()
                });
            // Blocks until the command is done, which is what makes opening the
            // file free: it happens while the command runs.
            let Ok(record) = rx.recv() else {
                return;
            };
            if let Some(mut file) = file {
                let _ = file.write_all(stats::format(&record).as_bytes());
            }
        });
        Self {
            tx: Some(tx),
            thread: Some(thread),
        }
    }

    /// Record one run and wait for it to reach the disk — one append to a file
    /// that has been open since the command started.
    pub fn finish(mut self, record: stats::Record) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(record);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Recorder {
    /// A recorder dropped without finishing — a command that panicked — closes
    /// the channel so the thread stops waiting for a record that is not coming.
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Read the log, or nothing when there is not one yet.
fn read(ctx: &Ctx) -> Result<Vec<stats::Record>> {
    match std::fs::read_to_string(&ctx.stats_path) {
        Ok(text) => Ok(stats::parse(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => bail!("reading {}: {e}", ctx.stats_path.display()),
    }
}

/// The tree, with what each command has cost, for `show` and for `improve`.
fn rows(ctx: &Ctx, command: &clap::Command) -> Result<Vec<stats::TreeRow>> {
    let totals = stats::totals(&read(ctx)?);
    Ok(stats::rows(&stats::tree(command), &totals))
}

/// `scriv stats show` — every command there is, with how often it has been run
/// and what one run costs.
pub fn show(ctx: &Ctx, command: &clap::Command) -> Result<()> {
    let rows = rows(ctx, command)?;

    let mut out = term::Listing::stdout();
    for line in stats::render(&rows, ctx.color()) {
        if !out.line(&line)? {
            return Ok(());
        }
    }
    out.finish()?;
    Ok(())
}

/// `scriv stats reset` — forget every run recorded so far.
pub fn reset(ctx: &Ctx, yes: bool) -> Result<()> {
    let records = read(ctx)?;
    if records.is_empty() {
        println!("nothing recorded yet");
        return Ok(());
    }

    match term::Confirm::resolve(yes) {
        term::Confirm::Assumed => {}
        term::Confirm::Ask => {
            let question = format!("Forget {} recorded runs?", records.len());
            let _waiting = stats::interacting();
            if !term::confirm(&question)? {
                println!("kept");
                return Ok(());
            }
        }
        term::Confirm::Impossible => bail!(
            "{} recorded runs would be forgotten; pass --yes to do it without a terminal to ask at",
            records.len()
        ),
    }

    // Emptied rather than removed: this run has the log open already, and a
    // file removed from under an open handle is one the next write brings
    // back with a hole where the rows were.
    match OpenOptions::new().write(true).open(&ctx.stats_path) {
        Ok(file) => file
            .set_len(0)
            .map_err(|e| anyhow::anyhow!("emptying {}: {e}", ctx.stats_path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => bail!("emptying {}: {e}", ctx.stats_path.display()),
    }
    println!("forgot {} recorded runs", records.len());
    Ok(())
}

/// The program `improve` hands the prompt to.
const CLAUDE: &str = "claude";

/// `scriv stats improve` — hand the statistics to Claude Code, in the directory
/// the user is standing in, and let it work on the commands worth the most.
///
/// Nothing is captured: Claude Code takes the terminal, as `scriv edit`'s
/// editor does.
pub fn improve(ctx: &Ctx, command: &clap::Command, dry_run: bool) -> Result<()> {
    let rows = rows(ctx, command)?;
    let prompt = stats::improve_prompt(&rows);

    if dry_run {
        println!("{prompt}");
        return Ok(());
    }
    if stats::by_value(&rows).is_empty() {
        bail!("nothing has been recorded yet, so there is nothing to improve");
    }
    if crate::cmd::config::on_path(CLAUDE).is_none() {
        bail!("`{CLAUDE}` is not on PATH — https://claude.com/product/claude-code");
    }

    ctx.log
        .info(&format!("handing {} rows to {CLAUDE}", rows.len()));
    let status = std::process::Command::new(CLAUDE)
        .arg(&prompt)
        .status()
        .map_err(|e| anyhow::anyhow!("running {CLAUDE}: {e}"))?;
    if !status.success() {
        return Err(crate::Reported(status.code().unwrap_or(1)).into());
    }
    Ok(())
}
