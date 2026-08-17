//! Interactive fuzzy selection, built in via [`skim`].
//!
//! The fuzzy finder is compiled into the binary — there is no `fzf` subprocess.
//! Every place that asks the user to choose goes through here.
//!
//! Items carry a separate [`SelectItem::label`] (shown and fuzzy-matched) and
//! value (returned on selection), so the selector can show a `~`-collapsed path
//! while still returning an absolute one.
//!
//! Rows arrive either all at once ([`select_one`], [`select_many`]) or as they
//! are discovered ([`select_one_streamed`], [`select_many_streamed`]).

use std::ops::Range;
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
/// Prefer [`Preview::Text`] whenever the data is already in hand. A
/// [`Preview::Command`] is only appropriate for work that is local and fast
/// (tens of milliseconds), because skim spawns it again on every move through
/// the list and does not kill the previous child.
pub enum Preview {
    /// Ready-made text; ANSI escapes are honoured.
    Text(String),
    /// A shell command, run only when the row is highlighted. skim exports
    /// `COLUMNS` and `ROWS` to it.
    Command(String),
    /// The row's own value, previewed as a file. Built when the row is
    /// highlighted rather than when it is created: a streamed walk can produce
    /// a million rows, and a command string on each is megabytes of panes
    /// nobody looks at.
    File,
}

/// Quote `arg` for the shell that runs a [`Preview::Command`], so a branch name
/// or path containing spaces or quotes cannot alter the command.
pub fn quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// The preview for a file: its contents, via `bat` when installed and `head`
/// otherwise, bounded to 200 lines.
pub fn file_preview(path: &str) -> Preview {
    Preview::Command(file_preview_cmd(path))
}

/// The preview for a directory: what is directly inside it, capped at 200 rows.
pub fn dir_preview(path: &str) -> Preview {
    let path = quote(path);
    Preview::Command(format!("ls -Ap -- {path} 2>&1 | head -n 200"))
}

/// The preview for a git checkout — a repository or one of its worktrees: the
/// current branch and working-tree state, then recent commits. Both commands
/// are local and take tens of milliseconds.
///
/// `--no-optional-locks` matters here: a plain `git status` rewrites the index,
/// so scrolling past a checkout would contend for its index lock.
pub fn checkout_preview(path: &str) -> Preview {
    let checkout = quote(path);
    Preview::Command(format!(
        "git --no-optional-locks -C {checkout} -c color.status=always status --short --branch \
         | head -n 10; \
         git --no-optional-locks -C {checkout} log --color=always --max-count=20 --date=relative \
         --format='%C(auto)%h%C(reset)  %C(blue)%an%C(reset)  %C(green)%ad%C(reset)  %s'"
    ))
}

/// The command behind [`Preview::File`] and [`file_preview`].
fn file_preview_cmd(path: &str) -> String {
    let path = quote(path);
    format!(
        "bat --color=always --style=plain --line-range=:200 -- {path} 2>/dev/null \
         || head -n 200 -- {path}"
    )
}

/// A stretch of a label drawn in its own colour, as a character range.
///
/// Where [`SelectItem::color`] says something about the whole row, a tint says
/// something about one column of it — so a row can carry two facts that are
/// true of different parts of it without either claiming the line.
pub struct Tint {
    pub range: Range<usize>,
    pub color: u8,
}

/// One choice in the selector: `label` is displayed and matched against,
/// [`SelectItem::value`] is returned when it is selected, `color` optionally
/// tints the row (an ANSI 256-colour index, so it respects the terminal
/// theme), and `preview` fills the preview pane while the row is highlighted.
pub struct SelectItem {
    pub label: String,
    /// Drawn dim, ahead of the label, and *not* matched against — for a column
    /// that identifies a row without being what one searches for, such as the
    /// date on a history entry.
    pub prefix: Option<String>,
    /// `None` when the value is the label itself. Read through
    /// [`SelectItem::value`].
    value: Option<String>,
    pub color: Option<u8>,
    pub tints: Vec<Tint>,
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
            tints: Vec::new(),
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
            tints: Vec::new(),
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

    /// Colour individual columns of the label. See [`Tint`].
    pub fn tints(mut self, tints: Vec<Tint>) -> Self {
        self.tints = tints;
        self
    }

    /// Show `preview` in the preview pane while this row is highlighted.
    pub fn preview(mut self, preview: Preview) -> Self {
        self.preview = Some(preview);
        self
    }
}

/// Colour of a [`SelectItem::prefix`]: ANSI 8, the terminal's own grey.
const PREFIX_COLOR: u8 = 8;

/// Recolour the character ranges in `tints`, leaving skim's match highlighting
/// alone: a span drawn in anything but `base` is a character the query matched,
/// and why the row is on screen at all outranks which column it sits in.
fn tinted<'a>(line: Line<'a>, tints: &[Tint], base: Style) -> Line<'a> {
    let color_at = |index: usize| {
        tints
            .iter()
            .find(|tint| tint.range.contains(&index))
            .map(|tint| tint.color)
    };
    let style = |color: Option<u8>| match color {
        Some(color) => base.fg(Color::Indexed(color)),
        None => base,
    };

    let mut out = Line::default();
    let mut at = 0;
    for span in line.spans {
        let width = span.content.chars().count();
        if span.style != base {
            out.push_span(span);
            at += width;
            continue;
        }
        // One span per run of characters sharing a colour, rather than per
        // character: a row is redrawn on every keystroke.
        let mut run = String::new();
        let mut color = None;
        for (offset, ch) in span.content.chars().enumerate() {
            let next = color_at(at + offset);
            if next != color && !run.is_empty() {
                out.push_span(Span::styled(std::mem::take(&mut run), style(color)));
            }
            color = next;
            run.push(ch);
        }
        if !run.is_empty() {
            out.push_span(Span::styled(run, style(color)));
        }
        at += width;
    }
    out
}

/// Bridges a [`SelectItem`] to skim.
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
        if let Some(idx) = self.item.color {
            context.base_style = context.base_style.fg(Color::Indexed(idx));
        }

        let base = context.base_style;
        let mut line = context.to_line(self.text());
        if !self.item.tints.is_empty() {
            line = tinted(line, &self.item.tints, base);
        }

        let Some(prefix) = &self.item.prefix else {
            return line;
        };

        // Prepended as a span rather than folded into the label, so
        // `to_line`'s highlight positions still index the matched text.
        let mut out = Line::default();
        out.push_span(Span::styled(
            prefix.clone(),
            Style::default().fg(Color::Indexed(PREFIX_COLOR)),
        ));
        out.spans.extend(line.spans);
        out
    }

    fn preview(&self, _context: PreviewContext) -> ItemPreview {
        match &self.item.preview {
            Some(Preview::Text(text)) => ItemPreview::AnsiText(text.clone()),
            Some(Preview::Command(cmd)) => ItemPreview::Command(cmd.clone()),
            Some(Preview::File) => ItemPreview::Command(file_preview_cmd(self.item.value())),
            // Blank rather than `Global`, which would run the empty global
            // preview command.
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

/// The key that asks a reloadable selector to go and get fresher rows. It
/// displaces skim's own `ctrl-r`, so only the selectors over remote data offer
/// it.
pub const REFRESH_KEY: &str = "ctrl-r";

/// Fetches a fresh set of rows. Called on a background thread, so it may block
/// for as long as the network takes.
///
/// Infallible by construction: a reload that fails should hand back the old
/// rows and remember the error, which only the caller can do.
pub type Reload = Box<dyn FnMut() -> Vec<SelectItem> + Send>;

/// [`select_one`], with [`REFRESH_KEY`] bound to reloading the list in place.
///
/// The selector does not close and reopen: `reload` runs on a background thread
/// while skim keeps drawing, and the query, cursor and preview pane are never
/// disturbed.
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

        // `ReaderControl::kill` *busy-waits* on this counter, so the counted
        // thread has to decrement promptly when asked. The reload itself
        // therefore runs on a second, uncounted thread that this one waits on.
        // A reload started while another runs does not cancel it; it queues.
        components_to_stop.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let (tx_rows, rx_rows) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rows = (reload.lock().expect("reload closure poisoned"))();
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
            // Dropping this is what stops skim's spinner, so it has to happen
            // before the count drops.
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

/// Select one item, or accept what the user typed when nothing matched — for
/// lists that are suggestions rather than the whole truth. An empty query with
/// no selection is a cancel, not an empty answer.
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

/// [`select_one`] over rows that arrive as they are found. No row exists when
/// the pane is configured, so whether previews are offered is stated up front
/// in `preview`.
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
/// let one sit unsent. The interval is under a frame, so batching costs nothing
/// visible.
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
                        // A closed channel means the selector is gone; stop
                        // walking rather than finish the tree for nobody.
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
    // A clearer message than skim's raw "Device not configured". Command
    // substitution still has a tty on stdin/stderr, so it is allowed.
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() && !std::io::stderr().is_terminal() {
        anyhow::bail!("interactive selection needs a terminal");
    }

    // skim does not stop when its input ends, so a selector whose terminal has
    // gone spins at 100% CPU until something kills the process.
    let _watch = crate::term::watch_for_hangup();

    let mut builder = SkimOptionsBuilder::default();
    builder
        .height(cfg.height.clone())
        .prompt(format!("{prompt}> "))
        .reverse(true)
        .multi(multi);

    // The per-item `preview()` supplies the content, so an empty command
    // string is all it takes to turn the pane on.
    if cfg.preview && feed.preview {
        builder
            .preview("")
            .preview_window(cfg.preview_window.as_str());
    }

    if !run.query.is_empty() {
        builder.query(run.query.to_string());
    }

    // `no_clear_if_empty` keeps a quick reload from flickering; a slow one
    // still empties the list, which is what the busy header is for.
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

    let (tx, rx): (SkimItemSender, SkimItemReceiver) = unbounded();
    feed.send(tx)?;

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
/// A full-screen selector takes the alternate screen and needs no row.
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
/// duration, so this is what distinguishes fetching from "there are none".
const BUSY_HEADER: &str = "⟳ refreshing…";

/// The skim bindings that turn [`REFRESH_KEY`] into a reload of the item list.
/// The second is skim's `load` event, which restores the idle header when a
/// read finishes.
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

    #[test]
    fn a_prefix_is_drawn_dim_before_the_label() {
        let item = SelectItem::plain("git status").prefix("2026-07-30 13:57  ");
        let line = rendered(item, vec![]);
        assert_eq!(text_of(&line), "2026-07-30 13:57  git status");
        assert_eq!(line.spans[0].content, "2026-07-30 13:57  ");
        assert_eq!(line.spans[0].style.fg, Some(Color::Indexed(PREFIX_COLOR)));
    }

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

    #[test]
    fn an_item_without_a_prefix_is_unchanged() {
        let line = rendered(SelectItem::plain("git status"), vec![]);
        assert_eq!(text_of(&line), "git status");
    }

    /// The characters drawn in `color`, wherever the renderer put them.
    fn painted(line: &Line, color: u8) -> String {
        line.spans
            .iter()
            .filter(|s| s.style.fg == Some(Color::Indexed(color)))
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn a_tint_colours_its_range_and_nothing_else() {
        let item = SelectItem::plain("✓ billing-api  private").tints(vec![
            Tint {
                range: 0..1,
                color: 2,
            },
            Tint {
                range: 15..22,
                color: 3,
            },
        ]);
        let line = rendered(item, vec![]);
        assert_eq!(text_of(&line), "✓ billing-api  private");
        assert_eq!(painted(&line, 2), "✓");
        assert_eq!(painted(&line, 3), "private");
    }

    /// A tinted column that the query hit is still shown as matched: the
    /// highlight says why the row survived the search, which the colour of the
    /// column it happens to sit in must not paint over.
    #[test]
    fn match_highlighting_survives_a_tint() {
        let item = SelectItem::plain("✓ billing-api  private").tints(vec![Tint {
            range: 15..22,
            color: 3,
        }]);
        // Character 15: the `p` of `private`.
        let line = rendered(item, vec![15]);
        assert_eq!(painted(&line, 1), "p", "{line:?}");
        assert_eq!(painted(&line, 3), "rivate", "{line:?}");
    }

    #[test]
    fn an_item_without_tints_is_unchanged() {
        let line = rendered(SelectItem::plain("git status"), vec![]);
        assert_eq!(line.spans.len(), 1, "{line:?}");
    }

    #[test]
    fn quotes_plain_values() {
        assert_eq!(quote("main"), "'main'");
        assert_eq!(quote("/home/u/my repo"), "'/home/u/my repo'");
    }

    #[test]
    fn only_a_full_height_selector_keeps_off_the_display() {
        assert!(!draws_inline("100%"));
        assert!(!draws_inline(" 100% "));
        for height in ["50%", "99%", "20", "-2", "", "garbage"] {
            assert!(draws_inline(height), "{height:?} does not draw inline");
        }
    }

    #[test]
    fn a_bare_hundred_is_not_full_height() {
        assert!(draws_inline("100"));
    }

    #[test]
    fn a_row_is_taken_only_when_there_is_a_terminal() {
        use std::io::IsTerminal;
        assert_eq!(room_for("50%").is_taken(), std::io::stderr().is_terminal());
    }

    #[test]
    fn a_full_height_selector_takes_no_row() {
        assert!(!room_for("100%").is_taken());
    }

    #[test]
    fn quotes_escape_embedded_single_quotes() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("'; rm -rf /; '"), r"''\''; rm -rf /; '\'''");
    }

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

    #[test]
    fn finishing_a_read_restores_the_idle_header() {
        let restore = refresh_binds()
            .into_iter()
            .find(|b| b.starts_with("load:"))
            .expect("nothing puts the header back");
        assert!(restore.contains(IDLE_HEADER), "{restore}");
    }

    /// skim splits bindings on `,` and ends an argument at `)`.
    #[test]
    fn header_text_cannot_break_the_binding_syntax() {
        for header in [IDLE_HEADER, BUSY_HEADER] {
            assert!(!header.contains(','), "{header:?} would split the binding");
            assert!(!header.contains(')'), "{header:?} would end the argument");
        }
    }

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

        // The closed channel stops skim's spinner; the zeroed counter tells it
        // the collector has stopped. Both have to happen.
        assert!(rx.recv().is_err(), "the source channel was left open");
        while components.load(Ordering::SeqCst) != 0 {
            std::thread::yield_now();
        }
    }

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
