//! Interactive fuzzy selection, built in via [`skim`].
//!
//! The fuzzy finder is compiled into the binary — there is no `fzf` subprocess
//! and no external dependency. Every place that asks the user to choose a path
//! goes through here, so selection looks and behaves the same everywhere.
//!
//! Items carry a separate [`SelectItem::label`] (shown and fuzzy-matched) and
//! value (returned on selection), so the selector can show a `~`-collapsed path
//! or a group tag while still returning an absolute path.
//!
//! Rows can arrive either all at once ([`select_one`], [`select_many`]) or as they
//! are discovered ([`select_one_streamed`], [`select_many_streamed`]) — the latter
//! is what makes a walk of a large tree usable, since the selector opens on the
//! first rows instead of the last.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use skim::prelude::*;

use crate::config::SelectorConfig;
use crate::term;

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

/// One choice in the selector: `label` is displayed and matched against,
/// [`SelectItem::value`] is returned when it is selected, `color` optionally
/// tints the row (an ANSI 256-colour index, so it respects the terminal
/// theme), and `preview` fills the preview pane while the row is highlighted.
pub struct SelectItem {
    pub label: String,
    /// Drawn dim, ahead of the label, and *not* matched against.
    ///
    /// For a column that identifies a row without being what you search for —
    /// the date on a history entry. Putting it in the label instead would make
    /// it searchable, and since a date is digits at the start of every row,
    /// typing a `3` would rank four thousand timestamps above the command you
    /// were reaching for.
    pub prefix: Option<String>,
    /// `None` when the value is the label itself, which is the common case for
    /// path rows — worth not storing twice when a walk streams in a million of
    /// them. Read it through [`SelectItem::value`].
    value: Option<String>,
    pub color: Option<u8>,
    pub preview: Option<Preview>,
}

impl SelectItem {
    /// An item whose displayed label is also its returned value.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            label: text.into(),
            prefix: None,
            value: None,
            color: None,
            preview: None,
        }
    }

    /// An item with a distinct display label and returned value.
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            prefix: None,
            value: Some(value.into()),
            color: None,
            preview: None,
        }
    }

    /// Draw `prefix` dim, ahead of the label, outside what the query matches.
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
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

/// Colour of a [`SelectItem::prefix`]: ANSI 8, the terminal's own grey, so the
/// column reads as context beside the command rather than competing with it.
const PREFIX_COLOR: u8 = 8;

/// Bridges a [`SelectItem`] to skim: `text()` drives display and matching,
/// `output()` is what a selection yields, `display()` tints the row, and
/// `preview()` fills the preview pane.
struct SkItem {
    item: SelectItem,
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

        let Some(prefix) = &self.item.prefix else {
            return context.to_line(self.text());
        };

        // The prefix is drawn here rather than folded into the label because
        // `to_line`'s highlight positions are indices into the matched text.
        // Prepending a span leaves those indices — and so the highlighting —
        // exactly where they belong, over the label alone.
        let mut line = Line::default();
        line.push_span(Span::styled(
            prefix.clone(),
            Style::default().fg(Color::Indexed(PREFIX_COLOR)),
        ));
        line.spans.extend(context.to_line(self.text()).spans);
        line
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

/// Select exactly one item, returning its value. Returns [`Cancelled`] as an
/// error on cancel; the caller decides whether that is a silent exit or a
/// failure.
pub fn select_one(items: Vec<SelectItem>, prompt: &str, cfg: &SelectorConfig) -> Result<String> {
    one(Feed::batch(items), prompt, cfg)
}

/// [`select_one`], opened with `query` already in the search box.
///
/// For a selector reached part-way through typing — ctrl-r after half a command —
/// where retyping what is already on the line to narrow the list is precisely
/// the work the selector was opened to save. An empty `query` is the ordinary
/// case and starts the selector on everything.
pub fn select_one_queried(
    items: Vec<SelectItem>,
    prompt: &str,
    query: &str,
    cfg: &SelectorConfig,
) -> Result<String> {
    let run = Run {
        prompt,
        multi: false,
        query,
        reload: None,
    };
    run_selector(Feed::batch(items), run, cfg)?
        .values
        .into_iter()
        .next()
        .ok_or_else(|| Cancelled.into())
}

/// The key that asks a reloadable selector to go and get fresher rows.
///
/// This displaces skim's own `ctrl-r` (rotate between matching modes), which is
/// why only the selectors over remote data offer it: those are the lists that go
/// stale while you look at them, and re-reading them is worth more there than
/// switching to regex matching.
pub const REFRESH_KEY: &str = "ctrl-r";

/// Fetches a fresh set of rows. Called on a background thread, so it may block
/// for as long as the network takes.
///
/// Infallible by construction: a reload that fails is the caller's to explain,
/// and the useful answer is almost always "keep showing what we had", which
/// only the caller can rebuild. Hand back the old rows and remember the error.
pub type Reload = Box<dyn FnMut() -> Vec<SelectItem> + Send>;

/// [`select_one`], with [`REFRESH_KEY`] bound to reloading the list in place.
///
/// The selector does not close and reopen: `reload` runs on a background thread
/// while skim keeps drawing, with its own reading spinner turning next to the
/// row count, and the rows already on screen stay there until the new ones
/// arrive. The query, the cursor and the preview pane are never disturbed —
/// there is nothing to restore, because nothing was torn down.
///
/// This works by handing skim a [`CommandCollector`] of scriv's own. skim's
/// `reload` action clears the item pool and asks the collector for a new
/// source; the stock collector runs a shell command, and this one calls
/// `reload` instead. So the refresh rides skim's real reload path — including
/// the spinner and the "still reading" state — without a shell in sight.
pub fn select_one_reloading(
    items: Vec<SelectItem>,
    prompt: &str,
    cfg: &SelectorConfig,
    reload: Reload,
) -> Result<String> {
    let run = Run {
        prompt,
        multi: false,
        query: "",
        reload: Some(reload),
    };
    run_selector(Feed::batch(items), run, cfg)?
        .values
        .into_iter()
        .next()
        .ok_or_else(|| Cancelled.into())
}

/// The [`CommandCollector`] behind [`select_one_reloading`]: skim thinks it is
/// running a command, and it is calling a closure.
struct ReloadCollector {
    reload: Arc<Mutex<Reload>>,
}

/// How often the counted thread looks up from waiting to see whether skim has
/// asked it to stop.
const RELOAD_POLL: Duration = Duration::from_millis(20);

impl CommandCollector for ReloadCollector {
    fn invoke(
        &mut self,
        _cmd: &str,
        components_to_stop: Arc<AtomicUsize>,
    ) -> (SkimItemReceiver, Sender<i32>) {
        let (tx_item, rx_item): (SkimItemSender, SkimItemReceiver) = unbounded();
        let (tx_interrupt, rx_interrupt) = unbounded::<i32>();
        let reload = Arc::clone(&self.reload);

        // The counter is skim's only view of whether this collector has
        // stopped, and `ReaderControl::kill` *busy-waits* on it. Whatever this
        // thread does, it has to decrement quickly when asked — so the reload
        // itself runs on a second, uncounted thread and this one only waits for
        // it, giving up the moment skim interrupts.
        //
        // A reload started while another is still running therefore does not
        // cancel it: the first finishes into a channel nobody is reading, and
        // the second waits its turn on whatever the caller's closure locks.
        // Pressing the key three times means three fetches, one after another,
        // and a selector that stays responsive throughout — which is a better
        // trade than killing a `git fetch` halfway.
        components_to_stop.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let (tx_rows, rx_rows) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rows = (reload.lock().expect("reload closure poisoned"))();
                // The receiver is gone if skim gave up waiting; the work is
                // finished either way and its result is simply dropped.
                let _ = tx_rows.send(rows);
            });

            loop {
                if matches!(rx_interrupt.try_recv(), Ok(Some(_)) | Err(_)) {
                    break;
                }
                match rx_rows.recv_timeout(RELOAD_POLL) {
                    Ok(rows) => {
                        let _ = tx_item.send(rows.into_iter().map(into_skim).collect());
                        break;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    // The worker panicked; leave the list as it is.
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // Dropping `tx_item` here is what tells skim the read is over and
            // stops its spinner, so it must happen before the count drops.
            drop(tx_item);
            components_to_stop.fetch_sub(1, Ordering::SeqCst);
        });

        (rx_item, tx_interrupt)
    }
}

/// What [`select_one_or_query`] came back with.
pub enum Choice {
    /// A row the user selected.
    Item(String),
    /// What the user typed, when it matched no row.
    Query(String),
}

/// Select one item, or accept what the user typed when nothing matched.
///
/// For lists that are suggestions rather than the whole truth. scriv knows the
/// GitHub owners you have already cloned from and the ones in your config, but
/// it cannot know the one you are about to clone from for the first time — and
/// a selector that refuses to accept an unlisted answer would make the common
/// case convenient by making the uncommon case impossible.
///
/// An empty query with no selection is a cancel, not an empty answer.
pub fn select_one_or_query(
    items: Vec<SelectItem>,
    prompt: &str,
    cfg: &SelectorConfig,
) -> Result<Choice> {
    let (values, query) = run_with_query(Feed::batch(items), prompt, false, cfg)?;
    if let Some(value) = values.into_iter().next() {
        return Ok(Choice::Item(value));
    }
    match query.trim() {
        "" => Err(Cancelled.into()),
        typed => Ok(Choice::Query(typed.to_string())),
    }
}

/// Select zero or more items, returning their values. An empty result means the
/// user selected nothing; cancelling still yields [`Cancelled`].
pub fn select_many(
    items: Vec<SelectItem>,
    prompt: &str,
    cfg: &SelectorConfig,
) -> Result<Vec<String>> {
    run(Feed::batch(items), prompt, true, cfg)
}

/// [`select_one`] over rows that arrive as they are found.
///
/// The selector opens immediately and fills in as `items` yields, so the user can
/// type against the first rows while the rest are still being discovered.
/// Because no row exists yet when the pane is configured, whether previews are
/// offered has to be stated up front in `preview`.
pub fn select_one_streamed(
    items: impl IntoIterator<Item = SelectItem, IntoIter: Send + 'static>,
    prompt: &str,
    preview: bool,
    cfg: &SelectorConfig,
) -> Result<String> {
    one(Feed::stream(items, preview), prompt, cfg)
}

/// [`select_many`] over rows that arrive as they are found. See
/// [`select_one_streamed`].
pub fn select_many_streamed(
    items: impl IntoIterator<Item = SelectItem, IntoIter: Send + 'static>,
    prompt: &str,
    preview: bool,
    cfg: &SelectorConfig,
) -> Result<Vec<String>> {
    run(Feed::stream(items, preview), prompt, true, cfg)
}

fn one(feed: Feed, prompt: &str, cfg: &SelectorConfig) -> Result<String> {
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
    /// Every row known before the selector opens.
    Batch(Vec<SelectItem>),
    /// Rows produced over time, drained on a background thread.
    Stream(Box<dyn Iterator<Item = SelectItem> + Send>),
}

/// How many rows to accumulate before handing a batch to skim, and how long to
/// let one sit unsent. Sending each row on its own would put a million channel
/// round-trips in front of a walk; 15ms is under a frame, so the selector still
/// fills in as fast as the eye reads it.
const FEED_BATCH: usize = 512;
const FEED_INTERVAL: Duration = Duration::from_millis(15);

impl Feed {
    fn batch(items: Vec<SelectItem>) -> Self {
        let preview = items.iter().any(|item| item.preview.is_some());
        Self {
            rows: Rows::Batch(items),
            preview,
        }
    }

    fn stream(
        items: impl IntoIterator<Item = SelectItem, IntoIter: Send + 'static>,
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
                tx.send(batch)
                    .map_err(|e| anyhow!("feeding selector: {e}"))?;
            }
            Rows::Stream(items) => {
                // Detached: `Skim::run_with` has returned by the time the walk
                // notices the selector is gone, and there is nothing to join for.
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
                        // the tree on a selector that is no longer there.
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

fn into_skim(item: SelectItem) -> Arc<dyn SkimItem> {
    Arc::new(SkItem { item }) as Arc<dyn SkimItem>
}

/// Everything about one run of the selector except its rows.
struct Run<'a> {
    prompt: &'a str,
    /// Whether several rows can be selected.
    multi: bool,
    /// Text the input starts with.
    query: &'a str,
    /// Given one, [`REFRESH_KEY`] reloads the list through it.
    reload: Option<Reload>,
}

impl<'a> Run<'a> {
    fn new(prompt: &'a str, multi: bool) -> Self {
        Self {
            prompt,
            multi,
            query: "",
            reload: None,
        }
    }
}

/// What one run of the selector came back with.
struct Outcome {
    values: Vec<String>,
    /// What the user had typed — the answer itself when the list is a set of
    /// suggestions and nothing matched.
    query: String,
}

/// Drive skim over `feed` and return the selected values.
fn run(feed: Feed, prompt: &str, multi: bool, cfg: &SelectorConfig) -> Result<Vec<String>> {
    run_with_query(feed, prompt, multi, cfg).map(|(values, _)| values)
}

/// [`run`], also returning what the user had typed.
fn run_with_query(
    feed: Feed,
    prompt: &str,
    multi: bool,
    cfg: &SelectorConfig,
) -> Result<(Vec<String>, String)> {
    let out = run_selector(feed, Run::new(prompt, multi), cfg)?;
    Ok((out.values, out.query))
}

/// Drive skim over `feed`.
fn run_selector(feed: Feed, run: Run, cfg: &SelectorConfig) -> Result<Outcome> {
    let (prompt, multi) = (run.prompt, run.multi);
    // skim needs a terminal for its UI. Fail with a clear message rather than
    // skim's raw "Device not configured" when there is none (e.g. in a pipe
    // with no controlling tty). Command substitution — `cd (scriv repo sel)` —
    // still has a tty on stdin/stderr, so it is allowed.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() && !std::io::stderr().is_terminal() {
        anyhow::bail!("interactive selection needs a terminal");
    }

    // skim does not stop when its input ends, so a selector whose terminal has
    // gone spins at 100% CPU for as long as the process is left alive. Held for
    // exactly as long as the selector is open.
    let _watch = crate::term::watch_for_hangup();

    let mut builder = SkimOptionsBuilder::default();
    builder
        .height(cfg.height.clone())
        .prompt(format!("{prompt}> "))
        .reverse(true)
        .multi(multi);

    // The pane only exists when skim has a preview command; the per-item
    // `preview()` above supplies the actual content, so an empty string is
    // enough to turn it on. Skipped entirely when no item has a preview, so
    // selectors without one keep the full width.
    if cfg.preview && feed.preview {
        builder
            .preview("")
            .preview_window(cfg.preview_window.as_str());
    }

    if !run.query.is_empty() {
        builder.query(run.query.to_string());
    }

    // skim's own `reload` action, pointed at a collector that calls a closure
    // instead of running a shell command.
    //
    // `no_clear_if_empty` keeps skim from blanking the displayed rows the
    // instant the key is pressed, which is all it can do: a reload empties the
    // item pool by definition, so once the matcher runs again the list is empty
    // until the new rows land. A reload that returns quickly therefore never
    // flickers, and a slow one shows an empty list — which is what the busy
    // header is for, since "still fetching" and "no branches" look identical
    // otherwise.
    if let Some(reload) = run.reload {
        let collector = ReloadCollector {
            reload: Arc::new(Mutex::new(reload)),
        };
        builder
            .bind(refresh_binds())
            .header(IDLE_HEADER.to_string())
            .no_clear_if_empty(true)
            .cmd_collector(Rc::new(RefCell::new(collector)) as Rc<RefCell<dyn CommandCollector>>);
    }

    let options = builder
        .build()
        .map_err(|e| anyhow!("configuring selector: {e}"))?;

    // Feed the items through a channel; the sender is dropped when they run
    // out, which is how skim stops waiting for more.
    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    feed.send(tx)?;

    // Held until the selector is done with the terminal, however it ends —
    // selection, cancel, or an error out of skim.
    let _room = room_for(&cfg.height);
    let output = Skim::run_with(options, Some(rx)).map_err(|e| anyhow!("running selector: {e}"))?;

    if output.is_abort {
        return Err(Cancelled.into());
    }
    Ok(Outcome {
        values: output
            .selected_items
            .iter()
            .map(|item| item.output().to_string())
            .collect(),
        query: output.query,
    })
}

/// The row an inline selector opens on, so it never draws over the prompt.
///
/// skim's inline viewport starts on the row the cursor is on — and when the
/// selector is opened from a key binding, that is the last row of the shell's
/// prompt. skim draws over it and clears it on the way out, which a one-line
/// prompt survives because the shell redraws the whole thing afterwards. A
/// two-line prompt does not: only its last row is taken, so the selector appears
/// welded to the middle of a prompt whose first row is still sitting above it.
/// A [`term::ScratchRow`] moves the whole selector one row down, clear of it.
///
/// A full-screen selector takes the alternate screen and gives the display back
/// untouched, so it needs no row of its own.
fn room_for(height: &str) -> term::ScratchRow {
    if draws_inline(height) {
        term::ScratchRow::take()
    } else {
        term::ScratchRow::none()
    }
}

/// Whether skim will draw over the terminal it was launched in, rather than
/// taking the alternate screen.
///
/// Mirrors skim's own rule: `100%` is full-screen, every other height — a
/// percentage, a row count, or a negative offset — is an inline viewport.
fn draws_inline(height: &str) -> bool {
    height
        .trim()
        .strip_suffix('%')
        .and_then(|n| n.trim().parse::<u16>().ok())
        != Some(100)
}

/// The selector's header when it is showing what it has.
const IDLE_HEADER: &str = "ctrl-r to refresh";

/// The header while a reload is in flight. skim empties the list for the
/// duration of a reload — that is what `reload` means — so the header is what
/// distinguishes "fetching, one moment" from "there are no branches". Its
/// spinner keeps turning beside the row count throughout.
const BUSY_HEADER: &str = "⟳ refreshing…";

/// The skim bindings that turn [`REFRESH_KEY`] into a reload of the item list.
///
/// `reload` with no command of its own is skim's "read the source again"; the
/// source here is [`ReloadCollector`], so this is what calls the closure. The
/// second binding is skim's `load` event, which fires when a read finishes —
/// including the first one — and puts the idle header back without scriv having
/// to know when the reload landed.
fn refresh_binds() -> Vec<String> {
    vec![
        format!("{REFRESH_KEY}:reload+set-header({BUSY_HEADER})"),
        format!("load:set-header({IDLE_HEADER})"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render an item the way skim would, with `matches` standing in for a
    /// query that hit the given characters of the *label*.
    fn rendered(item: SelectItem, matches: Vec<usize>) -> Line<'static> {
        let sk = SkItem { item };
        let context = DisplayContext {
            score: 0,
            matches: Matches::CharIndices(matches),
            container_width: 80,
            base_style: Style::default(),
            matched_style: Style::default().fg(Color::Indexed(1)),
        };
        let line = sk.display(context);
        Line::from(
            line.spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style))
                .collect::<Vec<_>>(),
        )
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The prefix is drawn ahead of the label in the terminal's grey, so a date
    /// reads as context beside the command rather than competing with it.
    #[test]
    fn a_prefix_is_drawn_dim_before_the_label() {
        let item = SelectItem::plain("git status").prefix("2026-07-30 13:57  ");
        let line = rendered(item, vec![]);
        assert_eq!(text_of(&line), "2026-07-30 13:57  git status");
        assert_eq!(line.spans[0].content, "2026-07-30 13:57  ");
        assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(PREFIX_COLOR)));
    }

    /// Match highlighting has to land on the label. skim reports match
    /// positions as indices into the text it matched — the label — so folding
    /// the prefix into that text instead would shift every highlight right by
    /// the width of the date.
    #[test]
    fn highlighting_lands_on_the_label_not_the_prefix() {
        // Character 0 of the label: the `g` of `git`.
        let matched = Style::default().fg(Color::Indexed(1));
        let with = rendered(
            SelectItem::plain("git status").prefix("2026-07-30 13:57  "),
            vec![0],
        );
        let hit: String = with
            .spans
            .iter()
            .filter(|s| s.style.fg == matched.fg)
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(hit, "g", "highlight slid off the label: {with:?}");
    }

    /// A row with no prefix renders exactly as it did before there was one.
    #[test]
    fn an_item_without_a_prefix_is_unchanged() {
        let line = rendered(SelectItem::plain("git status"), vec![]);
        assert_eq!(text_of(&line), "git status");
    }

    #[test]
    fn quotes_plain_values() {
        assert_eq!(quote("main"), "'main'");
        assert_eq!(quote("/home/u/my repo"), "'/home/u/my repo'");
    }

    /// Only a full-height selector takes the alternate screen, and only that one
    /// leaves the display untouched on its own. Get this wrong in the generous
    /// direction and scriv reserves a row a full-screen selector never draws in,
    /// leaving a stray blank line above every prompt.
    #[test]
    fn only_a_full_height_selector_keeps_off_the_display() {
        assert!(!draws_inline("100%"));
        assert!(!draws_inline(" 100% "));
        for height in ["50%", "99%", "20", "-2", "", "garbage"] {
            assert!(draws_inline(height), "{height:?} does not draw inline");
        }
    }

    /// A row count of 100 is a hundred rows, not a hundred per cent — the `%`
    /// is what makes it full-screen.
    #[test]
    fn a_bare_hundred_is_not_full_height() {
        assert!(draws_inline("100"));
    }

    /// Taking a row means writing to the terminal, so it must not happen when
    /// there is no terminal there: a redirected run would otherwise emit a
    /// stray newline and a cursor-up escape into whatever is reading it.
    #[test]
    fn a_row_is_taken_only_when_there_is_a_terminal() {
        use std::io::IsTerminal;
        assert_eq!(room_for("50%").is_taken(), std::io::stderr().is_terminal());
    }

    /// A full-screen selector restores the display itself, so there is nothing
    /// to take however the run was launched.
    #[test]
    fn a_full_height_selector_takes_no_row() {
        assert!(!room_for("100%").is_taken());
    }

    /// A single quote in a branch name or path must not end the quoted string
    /// and let the rest be read as shell syntax.
    #[test]
    fn quotes_escape_embedded_single_quotes() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("'; rm -rf /; '"), r"''\''; rm -rf /; '\'''");
    }

    /// The binding has to be skim's own `reload`, not an `accept`: an accept
    /// closes the selector, which is the behaviour this exists to avoid. It also
    /// has to parse — a binding skim cannot read is dropped, and the key would
    /// silently do nothing.
    #[test]
    fn the_refresh_key_is_bound_to_a_reload() {
        let binds = refresh_binds();
        let (key, actions) = binds[0]
            .split_once(':')
            .expect("not a `key:action` binding");
        assert_eq!(key, REFRESH_KEY);
        assert!(
            actions.starts_with("reload"),
            "an accept would close the selector: {actions}"
        );
        assert!(actions.contains(BUSY_HEADER), "no busy header: {actions}");
    }

    /// The busy header has to be taken down again, or a selector sits there
    /// claiming to be refreshing long after it finished. skim's `load` event
    /// is what says a read is over.
    #[test]
    fn finishing_a_read_restores_the_idle_header() {
        let restore = refresh_binds()
            .into_iter()
            .find(|b| b.starts_with("load:"))
            .expect("nothing puts the header back");
        assert!(restore.contains(IDLE_HEADER), "{restore}");
    }

    /// Neither the parenthesis nor the comma may appear inside a binding's
    /// argument: skim splits bindings on `,` and ends an argument at `)`, so a
    /// header containing either would be parsed as something else entirely.
    #[test]
    fn header_text_cannot_break_the_binding_syntax() {
        for header in [IDLE_HEADER, BUSY_HEADER] {
            assert!(!header.contains(','), "{header:?} would split the binding");
            assert!(!header.contains(')'), "{header:?} would end the argument");
        }
    }

    /// The reload closure runs on a thread of the collector's making, and its
    /// rows have to come back through the channel skim is reading. Driving the
    /// collector directly is the only way to see that without a terminal.
    #[test]
    fn the_collector_hands_reloaded_rows_to_skim() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&calls);
        let mut collector = ReloadCollector {
            reload: Arc::new(Mutex::new(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                vec![SelectItem::plain("fresh")]
            }))),
        };

        let components = Arc::new(AtomicUsize::new(0));
        let (rx, _interrupt) = collector.invoke("", Arc::clone(&components));

        let batch = rx.recv().expect("the reload sent no rows");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].text(), "fresh");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // The channel closing is how skim learns the read is over and stops
        // its spinner; the counter reaching zero is how it learns the
        // collector has stopped. Both have to happen, or the selector is left
        // looking like it is still loading.
        assert!(rx.recv().is_err(), "the source channel was left open");
        while components.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }

    /// skim busy-waits on the component count when it kills a reader, so an
    /// interrupted reload has to give it up promptly rather than after the
    /// network has answered.
    #[test]
    fn an_interrupted_reload_stops_without_waiting_for_the_work() {
        let (release, blocked) = std::sync::mpsc::channel::<()>();
        let mut collector = ReloadCollector {
            reload: Arc::new(Mutex::new(Box::new(move || {
                // Stands in for a fetch that has not come back yet.
                let _ = blocked.recv();
                vec![SelectItem::plain("late")]
            }))),
        };

        let components = Arc::new(AtomicUsize::new(0));
        let (_rx, interrupt) = collector.invoke("", Arc::clone(&components));
        interrupt.send(1).expect("interrupt not delivered");

        // Without the uncounted worker thread this would hang until the
        // reload finished — exactly the freeze skim's spin would turn into a
        // pegged core.
        let start = Instant::now();
        while components.load(Ordering::SeqCst) != 0 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the collector waited for the reload it was told to abandon",
            );
            std::thread::yield_now();
        }
        drop(release);
    }
}
