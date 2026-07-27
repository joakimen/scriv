//! Interactive fuzzy selection, built in via [`skim`].
//!
//! The fuzzy finder is compiled into the binary — there is no `fzf` subprocess
//! and no external dependency. Every place that asks the user to choose a path
//! goes through here, so selection looks and behaves the same everywhere.
//!
//! Items carry a separate [`PickItem::label`] (shown and fuzzy-matched) and
//! [`PickItem::value`] (returned on selection), so the picker can show a
//! `~`-collapsed path or a group tag while still returning an absolute path.

use anyhow::{Result, anyhow};
use ratatui::style::Color;
use ratatui::text::Line;
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

/// One choice in the picker: `label` is displayed and matched against, `value`
/// is returned when it is selected, and `color` optionally tints the row (an
/// ANSI 256-colour index, so it respects the terminal theme).
pub struct PickItem {
    pub label: String,
    pub value: String,
    pub color: Option<u8>,
}

impl PickItem {
    /// An item whose displayed label is also its returned value.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            label: text.clone(),
            value: text,
            color: None,
        }
    }

    /// An item with a distinct display label and returned value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            color: None,
        }
    }

    /// Tint the row with an ANSI 256-colour index.
    pub fn color(mut self, color: u8) -> Self {
        self.color = Some(color);
        self
    }
}

/// Bridges a [`PickItem`] to skim: `text()` drives display and matching,
/// `output()` is what a selection yields, and `display()` tints the row.
struct SkItem {
    label: String,
    value: String,
    color: Option<u8>,
}

impl SkimItem for SkItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.label)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.value)
    }

    fn display(&self, mut context: DisplayContext) -> Line<'_> {
        // Tint the whole row in the group's colour; skim still overlays its
        // match highlighting on top via `to_line`.
        if let Some(idx) = self.color {
            context.base_style = context.base_style.fg(Color::Indexed(idx));
        }
        context.to_line(self.text())
    }
}

/// Pick exactly one item, returning its value. Returns [`Cancelled`] as an
/// error on cancel; the caller decides whether that is a silent exit or a
/// failure.
pub fn pick_one(items: Vec<PickItem>, prompt: &str, cfg: &PickerConfig) -> Result<String> {
    run(items, prompt, false, cfg)?
        .into_iter()
        .next()
        .ok_or_else(|| Cancelled.into())
}

/// Pick zero or more items, returning their values. An empty result means the
/// user selected nothing; cancelling still yields [`Cancelled`].
pub fn pick_many(items: Vec<PickItem>, prompt: &str, cfg: &PickerConfig) -> Result<Vec<String>> {
    run(items, prompt, true, cfg)
}

/// Drive skim over `items` and return the selected values.
fn run(items: Vec<PickItem>, prompt: &str, multi: bool, cfg: &PickerConfig) -> Result<Vec<String>> {
    // skim needs a terminal for its UI. Fail with a clear message rather than
    // skim's raw "Device not configured" when there is none (e.g. in a pipe
    // with no controlling tty). Command substitution — `cd (scriv repo pick)` —
    // still has a tty on stdin/stderr, so it is allowed.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() && !std::io::stderr().is_terminal() {
        anyhow::bail!("interactive selection needs a terminal");
    }

    let options = SkimOptionsBuilder::default()
        .height(cfg.height.clone())
        .prompt(format!("{prompt}> "))
        .reverse(true)
        .multi(multi)
        .build()
        .map_err(|e| anyhow!("configuring picker: {e}"))?;

    // Feed all items through a channel, then close it so skim stops waiting.
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    let batch: Vec<Arc<dyn SkimItem>> = items
        .into_iter()
        .map(|it| {
            Arc::new(SkItem {
                label: it.label,
                value: it.value,
                color: it.color,
            }) as Arc<dyn SkimItem>
        })
        .collect();
    tx.send(batch).map_err(|e| anyhow!("feeding picker: {e}"))?;
    drop(tx);

    let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow!("running picker: {e}"))?;

    if output.is_abort {
        return Err(Cancelled.into());
    }
    Ok(output
        .selected_items
        .iter()
        .map(|item| item.output().to_string())
        .collect())
}
