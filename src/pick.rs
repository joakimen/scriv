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

/// What the preview pane shows for the highlighted row.
///
/// Prefer [`Preview::Text`] whenever the data is already in hand: it renders
/// instantly, with nothing to spawn or wait for. A [`Preview::Command`] is only
/// appropriate for work that is local and fast (tens of milliseconds), because
/// skim spawns it again on every move through the list and does not kill the
/// previous child.
pub enum Preview {
    /// Ready-made text; ANSI escapes are honoured.
    Text(String),
    /// A shell command, run only when the row is highlighted — so the cost is
    /// paid per look, not per item. skim exports `COLUMNS` and `ROWS` to it,
    /// for tools that wrap their own output.
    Command(String),
}

/// Quote `arg` for the shell that runs a [`Preview::Command`], so a branch name
/// or path containing spaces or quotes cannot alter the command.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// One choice in the picker: `label` is displayed and matched against, `value`
/// is returned when it is selected, `color` optionally tints the row (an ANSI
/// 256-colour index, so it respects the terminal theme), and `preview` fills
/// the preview pane while the row is highlighted.
pub struct PickItem {
    pub label: String,
    pub value: String,
    pub color: Option<u8>,
    pub preview: Option<Preview>,
}

impl PickItem {
    /// An item whose displayed label is also its returned value.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            label: text.clone(),
            value: text,
            color: None,
            preview: None,
        }
    }

    /// An item with a distinct display label and returned value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            color: None,
            preview: None,
        }
    }

    /// Tint the row with an ANSI 256-colour index.
    pub fn color(mut self, color: u8) -> Self {
        self.color = Some(color);
        self
    }

    /// Show `preview` in the preview pane while this row is highlighted.
    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = Some(preview);
        self
    }
}

/// Bridges a [`PickItem`] to skim: `text()` drives display and matching,
/// `output()` is what a selection yields, `display()` tints the row, and
/// `preview()` fills the preview pane.
struct SkItem {
    label: String,
    value: String,
    color: Option<u8>,
    preview: Option<Preview>,
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

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        match &self.preview {
            Some(Preview::Text(text)) => ItemPreview::AnsiText(text.clone()),
            Some(Preview::Command(cmd)) => ItemPreview::Command(cmd.clone()),
            // Blank rather than `Global`, which would run the (empty) global
            // preview command for rows that have nothing to show.
            None => ItemPreview::Text(String::new()),
        }
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

    let mut builder = SkimOptionsBuilder::default();
    builder
        .height(cfg.height.clone())
        .prompt(format!("{prompt}> "))
        .reverse(true)
        .multi(multi);

    // The pane only exists when skim has a preview command; the per-item
    // `preview()` above supplies the actual content, so an empty string is
    // enough to turn it on. Skipped entirely when no item has a preview, so
    // pickers without one keep the full width.
    if cfg.preview && items.iter().any(|item| item.preview.is_some()) {
        builder
            .preview("")
            .preview_window(cfg.preview_window.as_str());
    }

    let options = builder
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
                preview: it.preview,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_plain_values() {
        assert_eq!(quote("main"), "'main'");
        assert_eq!(quote("/home/u/my repo"), "'/home/u/my repo'");
    }

    /// A single quote in a branch name or path must not end the quoted string
    /// and let the rest be read as shell syntax.
    #[test]
    fn quotes_escape_embedded_single_quotes() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("'; rm -rf /; '"), r"''\''; rm -rf /; '\'''");
    }
}
