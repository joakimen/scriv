//! Interactive fuzzy selection, built in via [`skim`].
//!
//! The fuzzy finder is compiled into the binary — there is no `fzf` subprocess
//! and no external dependency. Every place that asks the user to choose a path
//! goes through here, so selection looks and behaves the same everywhere.
//!
//! Items carry a separate [`PickItem::label`] (shown and fuzzy-matched) and
//! value (returned on selection), so the picker can show a `~`-collapsed path
//! or a group tag while still returning an absolute path.
//!
//! Rows can arrive either all at once ([`pick_one`], [`pick_many`]) or as they
//! are discovered ([`pick_one_streamed`], [`pick_many_streamed`]) — the latter
//! is what makes a walk of a large tree usable, since the picker opens on the
//! first rows instead of the last.

use std::time::{Duration, Instant};

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
    /// The row's own value, previewed as a file — [`file_preview`] built when
    /// the row is highlighted rather than when the row is created. A streamed
    /// walk can produce a million rows, and a formatted command string on each
    /// of them is a hundred megabytes of panes nobody looks at.
    File,
}

/// Quote `arg` for the shell that runs a [`Preview::Command`], so a branch name
/// or path containing spaces or quotes cannot alter the command.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The preview for a file: its contents.
///
/// `bat` renders them with syntax highlighting when it is installed; `head`
/// covers everyone else, and its error text is the preview when the path is
/// gone — which the known-files list expects to happen. Both are bounded to
/// 200 lines, so scrolling a list never reads a large file in full.
pub fn file_preview(path: &str) -> Preview {
    Preview::Command(file_preview_cmd(path))
}

/// The command behind [`Preview::File`] and [`file_preview`].
fn file_preview_cmd(path: &str) -> String {
    let path = quote(path);
    format!(
        "bat --color=always --style=plain --line-range=:200 -- {path} 2>/dev/null \
         || head -n 200 -- {path}"
    )
}

/// One choice in the picker: `label` is displayed and matched against,
/// [`PickItem::value`] is returned when it is selected, `color` optionally
/// tints the row (an ANSI 256-colour index, so it respects the terminal
/// theme), and `preview` fills the preview pane while the row is highlighted.
pub struct PickItem {
    pub label: String,
    /// `None` when the value is the label itself, which is the common case for
    /// path rows — worth not storing twice when a walk streams in a million of
    /// them. Read it through [`PickItem::value`].
    value: Option<String>,
    pub color: Option<u8>,
    pub preview: Option<Preview>,
}

impl PickItem {
    /// An item whose displayed label is also its returned value.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            label: text.into(),
            value: None,
            color: None,
            preview: None,
        }
    }

    /// An item with a distinct display label and returned value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: Some(value.into()),
            color: None,
            preview: None,
        }
    }

    /// What selecting this row yields — the label unless one was set apart.
    pub fn value(&self) -> &str {
        self.value.as_deref().unwrap_or(&self.label)
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
    item: PickItem,
}

impl SkimItem for SkItem {
    fn text(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.item.label)
    }

    fn output(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.item.value())
    }

    fn display(&self, mut context: DisplayContext) -> Line<'_> {
        // Tint the whole row in the group's colour; skim still overlays its
        // match highlighting on top via `to_line`.
        if let Some(idx) = self.item.color {
            context.base_style = context.base_style.fg(Color::Indexed(idx));
        }
        context.to_line(self.text())
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        match &self.item.preview {
            Some(Preview::Text(text)) => ItemPreview::AnsiText(text.clone()),
            Some(Preview::Command(cmd)) => ItemPreview::Command(cmd.clone()),
            Some(Preview::File) => ItemPreview::Command(file_preview_cmd(self.item.value())),
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
    one(Feed::batch(items), prompt, cfg)
}

/// The key that asks a refreshable picker to reload its rows.
///
/// This displaces skim's own `ctrl-r` (rotate between matching modes), which is
/// why only the pickers over remote data offer it: those are the lists that go
/// stale while you look at them, and re-reading them is worth more there than
/// switching to regex matching.
pub const REFRESH_KEY: &str = "ctrl-r";

/// The outcome of a picker that offers [`REFRESH_KEY`].
pub enum Picked {
    /// A row the user selected.
    Chosen(String),
    /// The user asked for fresher rows. `query` is what they had typed, so the
    /// reopened picker can start where they left off rather than making them
    /// type it again.
    Refresh { query: String },
}

/// [`pick_one`], with [`REFRESH_KEY`] bound to "close and tell the caller to
/// reload".
///
/// The reload itself is the caller's: only it knows whether fresh rows mean a
/// `git fetch` or a `gh pr list`, and doing the work between runs of the picker
/// rather than inside one keeps it off skim's preview thread, where a slow
/// command piles up copies of itself.
///
/// `query` pre-fills the input, which is how a refresh keeps its place.
pub fn pick_one_refreshable(
    items: Vec<PickItem>,
    prompt: &str,
    query: &str,
    cfg: &PickerConfig,
) -> Result<Picked> {
    let run = Run {
        prompt,
        multi: false,
        query,
        refreshable: true,
    };
    let out = run_picker(Feed::batch(items), &run, cfg)?;
    if out.refresh {
        return Ok(Picked::Refresh { query: out.query });
    }
    out.values
        .into_iter()
        .next()
        .map(Picked::Chosen)
        .ok_or_else(|| Cancelled.into())
}

/// What [`pick_one_or_query`] came back with.
pub enum Choice {
    /// A row the user selected.
    Item(String),
    /// What the user typed, when it matched no row.
    Query(String),
}

/// Pick one item, or accept what the user typed when nothing matched.
///
/// For lists that are suggestions rather than the whole truth. scriv knows the
/// GitHub owners you have already cloned from and the ones in your config, but
/// it cannot know the one you are about to clone from for the first time — and
/// a picker that refuses to accept an unlisted answer would make the common
/// case convenient by making the uncommon case impossible.
///
/// An empty query with no selection is a cancel, not an empty answer.
pub fn pick_one_or_query(items: Vec<PickItem>, prompt: &str, cfg: &PickerConfig) -> Result<Choice> {
    let (values, query) = run_with_query(Feed::batch(items), prompt, false, cfg)?;
    if let Some(value) = values.into_iter().next() {
        return Ok(Choice::Item(value));
    }
    match query.trim() {
        "" => Err(Cancelled.into()),
        typed => Ok(Choice::Query(typed.to_string())),
    }
}

/// Pick zero or more items, returning their values. An empty result means the
/// user selected nothing; cancelling still yields [`Cancelled`].
pub fn pick_many(items: Vec<PickItem>, prompt: &str, cfg: &PickerConfig) -> Result<Vec<String>> {
    run(Feed::batch(items), prompt, true, cfg)
}

/// [`pick_one`] over rows that arrive as they are found.
///
/// The picker opens immediately and fills in as `items` yields, so the user can
/// type against the first rows while the rest are still being discovered.
/// Because no row exists yet when the pane is configured, whether previews are
/// offered has to be stated up front in `preview`.
pub fn pick_one_streamed(
    items: impl IntoIterator<Item = PickItem, IntoIter: Send + 'static>,
    prompt: &str,
    preview: bool,
    cfg: &PickerConfig,
) -> Result<String> {
    one(Feed::stream(items, preview), prompt, cfg)
}

/// [`pick_many`] over rows that arrive as they are found. See
/// [`pick_one_streamed`].
pub fn pick_many_streamed(
    items: impl IntoIterator<Item = PickItem, IntoIter: Send + 'static>,
    prompt: &str,
    preview: bool,
    cfg: &PickerConfig,
) -> Result<Vec<String>> {
    run(Feed::stream(items, preview), prompt, true, cfg)
}

fn one(feed: Feed, prompt: &str, cfg: &PickerConfig) -> Result<String> {
    run(feed, prompt, false, cfg)?
        .into_iter()
        .next()
        .ok_or_else(|| Cancelled.into())
}

/// Rows to feed skim, and whether any of them can fill the preview pane.
struct Feed {
    rows: Rows,
    preview: bool,
}

enum Rows {
    /// Every row known before the picker opens.
    Batch(Vec<PickItem>),
    /// Rows produced over time, drained on a background thread.
    Stream(Box<dyn Iterator<Item = PickItem> + Send>),
}

/// How many rows to accumulate before handing a batch to skim, and how long to
/// let one sit unsent. Sending each row on its own would put a million channel
/// round-trips in front of a walk; 15ms is under a frame, so the picker still
/// fills in as fast as the eye reads it.
const FEED_BATCH: usize = 512;
const FEED_INTERVAL: Duration = Duration::from_millis(15);

impl Feed {
    fn batch(items: Vec<PickItem>) -> Self {
        let preview = items.iter().any(|item| item.preview.is_some());
        Self {
            rows: Rows::Batch(items),
            preview,
        }
    }

    fn stream(
        items: impl IntoIterator<Item = PickItem, IntoIter: Send + 'static>,
        preview: bool,
    ) -> Self {
        Self {
            rows: Rows::Stream(Box::new(items.into_iter())),
            preview,
        }
    }

    /// Hand the rows to `tx`, closing it when they run out — that is how skim
    /// learns the source is exhausted and stops showing itself as still
    /// reading.
    fn send(self, tx: SkimItemSender) -> Result<()> {
        match self.rows {
            Rows::Batch(items) => {
                let batch: Vec<Arc<dyn SkimItem>> = items.into_iter().map(into_skim).collect();
                tx.send(batch).map_err(|e| anyhow!("feeding picker: {e}"))?;
            }
            Rows::Stream(items) => {
                // Detached: `Skim::run_with` has returned by the time the walk
                // notices the picker is gone, and there is nothing to join for.
                std::thread::spawn(move || {
                    let mut batch: Vec<Arc<dyn SkimItem>> = Vec::with_capacity(FEED_BATCH);
                    let mut flushed = Instant::now();
                    for item in items {
                        batch.push(into_skim(item));
                        if batch.len() < FEED_BATCH && flushed.elapsed() < FEED_INTERVAL {
                            continue;
                        }
                        let full = std::mem::replace(&mut batch, Vec::with_capacity(FEED_BATCH));
                        // A closed channel means the user has selected or
                        // cancelled: stop walking rather than spend the rest of
                        // the tree on a picker that is no longer there.
                        if tx.send(full).is_err() {
                            return;
                        }
                        flushed = Instant::now();
                    }
                    if !batch.is_empty() {
                        let _ = tx.send(batch);
                    }
                });
            }
        }
        Ok(())
    }
}

fn into_skim(item: PickItem) -> Arc<dyn SkimItem> {
    Arc::new(SkItem { item }) as Arc<dyn SkimItem>
}

/// Everything about one run of the picker except its rows.
struct Run<'a> {
    prompt: &'a str,
    /// Whether several rows can be selected.
    multi: bool,
    /// Text the input starts with, so a refreshed picker keeps its place.
    query: &'a str,
    /// Offer [`REFRESH_KEY`] and report it when pressed.
    refreshable: bool,
}

impl<'a> Run<'a> {
    fn new(prompt: &'a str, multi: bool) -> Self {
        Self {
            prompt,
            multi,
            query: "",
            refreshable: false,
        }
    }
}

/// What one run of the picker came back with.
struct Outcome {
    values: Vec<String>,
    /// What the user had typed — the answer itself when the list is a set of
    /// suggestions and nothing matched.
    query: String,
    /// [`REFRESH_KEY`] was pressed, so there is no selection to speak of.
    refresh: bool,
}

/// Drive skim over `feed` and return the selected values.
fn run(feed: Feed, prompt: &str, multi: bool, cfg: &PickerConfig) -> Result<Vec<String>> {
    run_with_query(feed, prompt, multi, cfg).map(|(values, _)| values)
}

/// [`run`], also returning what the user had typed.
fn run_with_query(
    feed: Feed,
    prompt: &str,
    multi: bool,
    cfg: &PickerConfig,
) -> Result<(Vec<String>, String)> {
    let out = run_picker(feed, &Run::new(prompt, multi), cfg)?;
    Ok((out.values, out.query))
}

/// Drive skim over `feed` once.
fn run_picker(feed: Feed, run: &Run, cfg: &PickerConfig) -> Result<Outcome> {
    let (prompt, multi) = (run.prompt, run.multi);
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
    if cfg.preview && feed.preview {
        builder
            .preview("")
            .preview_window(cfg.preview_window.as_str());
    }

    if !run.query.is_empty() {
        builder.query(run.query.to_string());
    }

    // `accept(<key>)` is skim's `--expect`: it closes the picker and names the
    // key that did it, which is how a keystroke can mean something other than
    // "this row" without skim knowing what a refresh is.
    if run.refreshable {
        builder
            .bind(vec![refresh_bind()])
            .header(format!("{REFRESH_KEY} to refresh"));
    }

    let options = builder
        .build()
        .map_err(|e| anyhow!("configuring picker: {e}"))?;

    // Feed the items through a channel; the sender is dropped when they run
    // out, which is how skim stops waiting for more.
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    feed.send(tx)?;

    let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow!("running picker: {e}"))?;

    if output.is_abort {
        return Err(Cancelled.into());
    }
    Ok(Outcome {
        values: output
            .selected_items
            .iter()
            .map(|item| item.output().to_string())
            .collect(),
        refresh: accepted_with(&output.final_event, REFRESH_KEY),
        query: output.query,
    })
}

/// The skim binding that closes the picker and names [`REFRESH_KEY`] as the
/// reason — the two halves of the round trip, written once.
fn refresh_bind() -> String {
    format!("{REFRESH_KEY}:accept({REFRESH_KEY})")
}

/// Whether skim closed because `key` was pressed, rather than `enter`.
///
/// The key comes back inside the accept event as the string it was bound
/// under, which is why the binding above and this comparison use the same
/// [`REFRESH_KEY`] constant.
fn accepted_with(event: &Event, key: &str) -> bool {
    matches!(
        event,
        Event::Action(Action::Accept(Some(pressed))) if pressed == key
    )
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

    /// Enter and the refresh key both leave skim through `accept`; only the
    /// key that came with it says which happened. Reading that wrong turns a
    /// refresh into a selection of whatever row was highlighted.
    #[test]
    fn only_the_refresh_key_reads_as_a_refresh() {
        let accept = |key: Option<&str>| Event::Action(Action::Accept(key.map(str::to_string)));
        assert!(accepted_with(&accept(Some(REFRESH_KEY)), REFRESH_KEY));
        // Plain enter: accepted, but with no key attached.
        assert!(!accepted_with(&accept(None), REFRESH_KEY));
        assert!(!accepted_with(&accept(Some("ctrl-t")), REFRESH_KEY));
        assert!(!accepted_with(&Event::Action(Action::Abort), REFRESH_KEY));
    }

    /// skim has to be told both which key to close on and what to call it, and
    /// the name it reports back is what [`accepted_with`] matches. A binding
    /// that named a different key would close the picker and refresh nothing.
    #[test]
    fn the_binding_names_the_key_it_is_checked_against() {
        assert_eq!(refresh_bind(), "ctrl-r:accept(ctrl-r)");
        let reported = Event::Action(Action::Accept(Some(REFRESH_KEY.to_string())));
        assert!(accepted_with(&reported, REFRESH_KEY));
    }
}
