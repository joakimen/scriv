//! Interactive fuzzy selection, built in via [`skim`].
//!
//! The fuzzy finder is compiled into the binary — there is no `fzf` subprocess
//! and no external dependency. Every place that asks the user to choose a path
//! goes through here, so selection looks and behaves the same everywhere.

use std::io::Cursor;

use anyhow::{Result, anyhow};
use skim::prelude::*;

use crate::config::PickerConfig;

/// A user-cancelled selection (Esc / Ctrl-C). Distinct from "nothing matched"
/// so the caller can exit 130 without printing anything.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("selection cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Pick exactly one item. Returns [`Cancelled`] as an error on cancel; the
/// caller decides whether that is a silent exit or a failure.
pub fn pick_one(items: &[String], prompt: &str, cfg: &PickerConfig) -> Result<String> {
    run(items, prompt, false, cfg)?
        .into_iter()
        .next()
        .ok_or_else(|| Cancelled.into())
}

/// Pick zero or more items. An empty result means the user selected nothing;
/// cancelling still yields [`Cancelled`].
pub fn pick_many(items: &[String], prompt: &str, cfg: &PickerConfig) -> Result<Vec<String>> {
    run(items, prompt, true, cfg)
}

/// Drive skim over `items` and return the selected lines.
fn run(items: &[String], prompt: &str, multi: bool, cfg: &PickerConfig) -> Result<Vec<String>> {
    let options = SkimOptionsBuilder::default()
        .height(cfg.height.clone())
        .prompt(format!("{prompt}> "))
        .reverse(true)
        .multi(multi)
        .build()
        .map_err(|e| anyhow!("configuring picker: {e}"))?;

    // skim reads items from a channel fed by a background reader over this
    // buffer; no temp files, no subprocess.
    let input = items.join("\n");
    let source = SkimItemReader::default().of_bufread(Cursor::new(input));

    let output =
        Skim::run_with(options, Some(source)).map_err(|e| anyhow!("running picker: {e}"))?;

    if output.is_abort {
        return Err(Cancelled.into());
    }
    Ok(output
        .selected_items
        .iter()
        .map(|item| item.output().to_string())
        .collect())
}
