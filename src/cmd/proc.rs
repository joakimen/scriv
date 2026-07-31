//! `scriv proc` — list, pick and signal running processes.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};

use crate::pick::{PickItem, Preview};
use crate::proc::{self, Process, Signal};
use crate::{Ctx, Reported, pick, term};

/// The process table, minus what must never be offered.
///
/// One `ps` call serves the listing, the picker rows and every preview pane —
/// see [`proc::preview`] for why the previews are built from it rather than
/// asking again per row.
fn processes() -> Result<Vec<Process>> {
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
    let text = String::from_utf8_lossy(&output.stdout);
    // `id()` is this process; everything above it in the parent chain runs up
    // through the shell to the terminal, and none of it is killable material.
    Ok(proc::selectable(
        &proc::parse(&text),
        std::process::id() as i32,
    ))
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

/// `scriv proc pick` — fuzzy-select a process and print its pid.
pub fn pick(ctx: &Ctx) -> Result<()> {
    let procs = processes()?;
    if procs.is_empty() {
        bail!("no processes to pick from");
    }
    let choice = pick::pick_one(rows(&procs), "Pick a process", &ctx.config.picker)?;
    println!("{choice}");
    Ok(())
}

/// `scriv proc kill` — signal processes, by pid or interactively.
///
/// Several pids are signalled one at a time rather than in a single `kill`
/// call, so that one refusal — a process owned by another user, or one that
/// exited between the listing and the keystroke — is reported against the row
/// it belongs to and does not hide the others that worked.
pub fn kill(ctx: &Ctx, pids: &[i32], signal: Signal) -> Result<()> {
    let procs = processes()?;
    let targets = if pids.is_empty() {
        match choose(ctx, &procs, signal)? {
            Some(targets) => targets,
            None => return Ok(()),
        }
    } else {
        pids.to_vec()
    };
    if targets.is_empty() {
        return Ok(());
    }

    let mut failed = 0;
    for pid in targets {
        // The name is for the report only, so a process that has since exited
        // is still signalled — and `kill` is the one that gets to say it is
        // gone.
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

/// Open the picker over `procs`, returning the pids chosen, or `None` when the
/// picker was cancelled.
fn choose(ctx: &Ctx, procs: &[Process], signal: Signal) -> Result<Option<Vec<i32>>> {
    if procs.is_empty() {
        bail!("no processes to pick from");
    }
    // The prompt names the signal, because `--force` is not visible once the
    // picker has taken the screen and the two are not equally recoverable.
    let prompt = format!("Send {signal} to");
    let selected = match pick::pick_many(rows(procs), &prompt, &ctx.config.picker) {
        Ok(selected) => selected,
        Err(e) if e.is::<pick::Cancelled>() => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some(
        selected.iter().filter_map(|v| v.parse().ok()).collect(),
    ))
}

/// Picker rows: the aligned listing row, returning the pid.
///
/// Uncoloured whatever `--color` says — the picker is a terminal UI that only
/// ever draws on a terminal, and it tints its own rows.
fn rows(procs: &[Process]) -> Vec<PickItem> {
    let width = proc::user_width(procs);
    procs
        .iter()
        .map(|p| {
            PickItem::new(proc::status_row(p, width, false), p.pid.to_string())
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
