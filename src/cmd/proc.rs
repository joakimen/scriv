//! `scriv proc` — list, select and signal running processes.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};

use crate::proc::{self, Process, Signal};
use crate::select::{Preview, SelectItem};
use crate::{Ctx, Reported, select, term};

/// The whole process table, as `ps` reported it. One call serves the listing,
/// the selector rows and every preview pane.
///
/// Unfiltered, because the entries that must never be *offered* are the ones
/// [`proc::refuse`] has to recognise when a pid is named on the command line.
fn table() -> Result<Vec<Process>> {
    let output = Command::new("ps")
        .args(proc::PS_ARGS)
        .stdin(Stdio::null())
        .output()
        .map_err(spawn_error)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        bail!(if stderr.is_empty() {
            "`ps` failed".to_string()
        } else {
            stderr.to_string()
        });
    }
    Ok(proc::parse(&String::from_utf8_lossy(&output.stdout)))
}

/// This process, which is the root of the chain that must never be signalled.
fn self_pid() -> i32 {
    std::process::id() as i32
}

/// The process table, minus what must never be offered: scriv itself and
/// everything above it in the parent chain.
fn processes() -> Result<Vec<Process>> {
    Ok(proc::selectable(&table()?, self_pid()))
}

fn spawn_error(err: std::io::Error) -> anyhow::Error {
    match err.kind() {
        ErrorKind::NotFound => anyhow!("`ps` was not found on PATH"),
        _ => anyhow!(err).context("running ps"),
    }
}

/// `scriv proc ls` — print the running processes, busiest first.
pub fn ls(ctx: &Ctx, status: bool) -> Result<()> {
    let procs = processes()?;
    let width = proc::user_width(&procs);
    let mut out = term::Listing::stdout();
    for p in &procs {
        let row = if status {
            proc::status_row(p, width, ctx.color())
        } else {
            proc::plain_row(p)
        };
        if !out.line(&row)? {
            break;
        }
    }
    Ok(())
}

/// `scriv proc sel` — fuzzy-select a process and print its pid.
pub fn sel(ctx: &Ctx) -> Result<()> {
    let procs = processes()?;
    if procs.is_empty() {
        bail!("no processes to select from");
    }
    let choice = select::select_one(rows(&procs), "Select a process", &ctx.config.selector)?;
    println!("{choice}");
    Ok(())
}

/// `scriv proc kill` — signal processes, by pid or interactively.
///
/// Several pids are signalled one at a time, so a refusal is reported against
/// the row it belongs to and does not hide the ones that worked.
pub fn kill(ctx: &Ctx, pids: &[i32], signal: Signal) -> Result<()> {
    let table = table()?;
    let procs = proc::selectable(&table, self_pid());

    let targets = if pids.is_empty() {
        match choose(ctx, &procs, signal)? {
            Some(targets) => targets,
            None => return Ok(()),
        }
    } else {
        // Nothing filtered these, so the whole run is refused rather than the
        // bad pids skipped: a partly signalled list is the hardest outcome.
        let refused = proc::refuse(&table, self_pid(), pids);
        if let Some((pid, refusal)) = refused.first() {
            bail!("refusing to signal {pid}: {}", refusal.reason());
        }
        pids.to_vec()
    };
    if targets.is_empty() {
        return Ok(());
    }

    let mut failed = 0;
    for pid in targets {
        // The name is for the report only; `kill` says whether it is gone.
        let name = procs
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name().to_string());
        match send(signal, pid) {
            Ok(()) => match name {
                Some(name) => println!("sent {signal} to {pid} {name}"),
                None => println!("sent {signal} to {pid}"),
            },
            // `kill` has already said why on stderr, in its own wording.
            Err(()) => failed += 1,
        }
    }
    if failed > 0 {
        return Err(Reported(1).into());
    }
    Ok(())
}

/// Open the selector over `procs`, returning the pids chosen, or `None` when the
/// selector was cancelled.
fn choose(ctx: &Ctx, procs: &[Process], signal: Signal) -> Result<Option<Vec<i32>>> {
    if procs.is_empty() {
        bail!("no processes to select from");
    }
    // The prompt names the signal: `--force` is not visible once the selector
    // has taken the screen.
    let prompt = format!("Send {signal} to");
    let selected = match select::select_many(rows(procs), &prompt, &ctx.config.selector) {
        Ok(selected) => selected,
        Err(e) if e.is::<select::Cancelled>() => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(
        selected.iter().filter_map(|v| v.parse().ok()).collect(),
    ))
}

/// Selector rows: the aligned listing row, returning the pid. Uncoloured
/// whatever `--color` says, since the selector tints its own rows.
fn rows(procs: &[Process]) -> Vec<SelectItem> {
    let width = proc::user_width(procs);
    procs
        .iter()
        .map(|p| {
            SelectItem::new(proc::status_row(p, width, false), p.pid.to_string())
                .preview(Preview::Text(proc::preview(p)))
        })
        .collect()
}

/// Send `signal` to one pid, letting `kill` report its own failures.
fn send(signal: Signal, pid: i32) -> Result<(), ()> {
    let status = Command::new("kill")
        .arg(format!("-{}", signal.name()))
        .arg(pid.to_string())
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(()),
        Err(e) => {
            eprintln!("error: running kill: {e}");
            Err(())
        }
    }
}
