//! Notes: a tree of Markdown files — an Obsidian vault, or anything shaped like
//! one — read as rows in a selector.
//!
//! Everything here is pure. The walk, the file reads and the clock live in
//! [`cmd::note`](crate::cmd::note); this module turns the bytes they hand back
//! into a title, a set of tags, two dates, an aligned row and a rendered
//! preview.
//!
//! A note says what it is in its YAML front matter, which is the only metadata
//! this reads. Inline `#tags` in the body are deliberately not indexed: finding
//! them means scanning every byte of every note and then telling a tag apart
//! from a heading, a colour literal and a URL fragment, and a tag column that is
//! right most of the time is worse than one that is right always.

use std::path::{Path, PathBuf};

use crate::select::Tint;

/// How the columns of a note row are coloured — indices into the ANSI
/// 256-colour table, so they follow the terminal's own theme. Blue for the
/// folder, as every file listing colours a directory; magenta for tags, which
/// is the one hue no status in scriv already means something with.
///
/// The title carries no colour of its own: it is what the query matches, and
/// skim's match highlighting has to be the brightest thing on the row.
const FOLDER_COLOR: u8 = 4;
const TAG_COLOR: u8 = 5;
const OPEN_COLOR: u8 = 3;

/// The task column's two glyphs: how many are left, or that none are. Shapes
/// rather than colour alone, so the column still says something where the
/// terminal's palette does not.
const OPEN_TASK: &str = "\u{2610}";
const ALL_DONE: &str = "\u{2713}";

/// What a note's front matter said about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Front {
    /// `title:`, when the note names itself something other than its filename.
    pub title: Option<String>,
    /// `tags:`/`tag:`, in the order written, without a leading `#`.
    pub tags: Vec<String>,
    /// `created:`/`date:`, as local midnight on the day it names.
    pub created: Option<i64>,
}

/// One note, as a listing sees it.
///
/// Built by the shell from a directory entry, its metadata and the first few
/// kilobytes of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Absolute path — what a row returns and what an editor is handed.
    pub path: PathBuf,
    /// Path below the vault root, extension and all. The name a note is known
    /// by on the command line.
    pub rel: String,
    /// The directory directly below the root that the note is filed under, or
    /// empty for a note at the root itself. What `[note] labels` labels.
    pub dir: String,
    /// Last modified, in Unix seconds.
    pub modified: i64,
    /// Created, in Unix seconds. See [`created`].
    pub created: i64,
    pub front: Front,
    pub tasks: Tasks,
}

/// The checkboxes in a note's body: how much of what it asked for is done.
///
/// The one thing a Markdown note says about itself that is neither its name nor
/// its metadata, and the one worth a column: a vault is full of notes that are
/// finished and notes that are still owed something, and nothing else on the
/// row tells them apart.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tasks {
    pub open: usize,
    pub done: usize,
}

impl Tasks {
    pub fn total(self) -> usize {
        self.open + self.done
    }

    /// The column: how many are left, or a tick once none are.
    fn column(self) -> String {
        match (self.open, self.done) {
            (0, 0) => String::new(),
            (0, _) => ALL_DONE.to_string(),
            (open, _) => format!("{open}{OPEN_TASK}"),
        }
    }

    fn color(self) -> u8 {
        if self.open == 0 {
            DONE_COLOR
        } else {
            OPEN_COLOR
        }
    }
}

/// Count the checkboxes in a note's body. A task is a list item whose marker is
/// followed by `[ ]` or `[x]`, which is the form every Markdown renderer that
/// draws a checkbox agrees on.
pub fn count_tasks(body: &str) -> Tasks {
    let mut tasks = Tasks::default();
    let mut fenced = false;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        // A checkbox inside a fence is an example of one, not one.
        if fenced {
            continue;
        }
        let Some((_, after)) = bullet(trimmed) else {
            continue;
        };
        match ticked(after) {
            Some(true) => tasks.done += 1,
            Some(false) => tasks.open += 1,
            None => {}
        }
    }
    tasks
}

/// Whether the text opens with a ticked box, an empty one, or neither.
fn ticked(text: &str) -> Option<bool> {
    let mut chars = text.strip_prefix('[')?.chars();
    let mark = chars.next()?;
    chars.next().filter(|c| *c == ']')?;
    match mark {
        ' ' => Some(false),
        'x' | 'X' => Some(true),
        _ => None,
    }
}

impl Note {
    /// What the note calls itself: its front matter's `title`, else the
    /// filename without its extension. Obsidian links notes by filename, so the
    /// filename is the name unless the note says otherwise.
    pub fn title(&self) -> &str {
        match &self.front.title {
            Some(title) => title,
            None => stem(&self.rel),
        }
    }

    /// The group column: the label [`Note::dir`] carries, or the directory's
    /// own name when it carries none.
    ///
    /// Unlike `repo`, which writes [`UNLABELLED`](crate::config::UNLABELLED)
    /// for an owner with no label, an unlabelled directory names itself here.
    /// A repository row still carries its owner in the path beside the label
    /// column; a note row does not, and a vault of five directories with two of
    /// them labelled would otherwise show three rows that say only `-`.
    pub fn group<'a>(&'a self, labels: &'a crate::config::NoteConfig) -> &'a str {
        labels.label_of(&self.dir).unwrap_or(&self.dir)
    }

    /// Whether the group column is a configured label rather than a bare
    /// directory name — which is what decides whether it takes a colour.
    pub fn labelled(&self, labels: &crate::config::NoteConfig) -> bool {
        labels.label_of(&self.dir).is_some()
    }

    /// The directories the note is filed under that the group column has not
    /// already named.
    ///
    /// Which ones those are depends on the group: a *label* is a word of the
    /// user's own — `work` says nothing about which directory — so the whole
    /// path below the root is still news. An unlabelled group is the directory
    /// itself, already drawn one column to the left, so only what sits below it
    /// is. Between them the two columns spell out the note's path exactly once.
    pub fn folder<'a>(&'a self, cfg: &'a crate::config::NoteConfig) -> &'a str {
        let below = match self.labelled(cfg) {
            true => &self.rel[..],
            // The separator too, which is why this is not `dir.len()`.
            false => &self.rel[(self.dir.len() + 1).min(self.rel.len())..],
        };
        match below.rfind('/') {
            Some(at) => &below[..at],
            None => "",
        }
    }

    /// The tags column: every tag, `#`-prefixed, space-separated.
    pub fn tag_column(&self) -> String {
        self.front
            .tags
            .iter()
            .map(|tag| format!("#{tag}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The directory directly below `root` that `rel` is filed under, or empty for
/// a path at the root itself.
pub fn top_dir(rel: &str) -> &str {
    match rel.find('/') {
        Some(at) => &rel[..at],
        None => "",
    }
}

/// When a note was created: what its front matter says, else what the
/// filesystem says.
///
/// The front matter wins because the filesystem's answer is so often wrong — a
/// vault that arrives by `git clone`, by a sync client or by a copy has every
/// note born the same afternoon, and a column that says that of five hundred
/// notes says nothing. `birth` is `None` on a filesystem that does not record
/// one, and the modification time is the last resort: nothing is newer than
/// itself.
pub fn created(front: Option<i64>, birth: Option<i64>, modified: i64) -> i64 {
    front.or(birth).unwrap_or(modified)
}

/// The filename, without its directories or its extension.
fn stem(rel: &str) -> &str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match name.rfind('.') {
        // A leading dot is the whole name, not an extension.
        Some(at) if at > 0 => &name[..at],
        _ => name,
    }
}

// --- front matter -----------------------------------------------------------

/// Split `text` into its YAML front matter block and the body below it.
///
/// The block is what sits between a first line of exactly `---` and the next
/// line of exactly `---` or `...`. An unterminated block is not one: a note
/// opening with a horizontal rule would otherwise lose its entire body.
pub fn split_front_matter(text: &str) -> (Option<&str>, &str) {
    let Some(after) = open_fence(text) else {
        return (None, text);
    };
    let mut at = 0;
    for line in after.split_inclusive('\n') {
        if matches!(line.trim_end_matches(['\n', '\r']), "---" | "...") {
            return (Some(&after[..at]), &after[at + line.len()..]);
        }
        at += line.len();
    }
    (None, text)
}

/// What follows the opening `---` line, when that is how `text` begins.
fn open_fence(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("---")?;
    rest.strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
}

/// Read a front matter block.
///
/// A deliberately small reader rather than a YAML parser: it takes top-level
/// `key: value` lines and the block or flow sequences under them, and skips
/// everything else — nested mappings, anchors, multi-line scalars. The three
/// keys it looks for are a scalar or a list of scalars in every vault that
/// writes them, and a YAML dependency would parse a great deal this never
/// reads.
///
/// `offset` dates a `created:` value, which names a day rather than an instant:
/// the day is the writer's own, so it becomes local midnight rather than UTC.
pub fn parse_front(block: &str, offset: time::UtcOffset) -> Front {
    let mut front = Front::default();
    // Whether the block sequence being read is the one `tags:` opened.
    let mut in_tags = false;

    for raw in block.lines() {
        let line = raw.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = sequence_item(trimmed) {
            if in_tags {
                push_tag(&mut front.tags, item);
            }
            continue;
        }
        // Indented and not a sequence item: part of a mapping this does not
        // read, rather than a key of its own.
        if line.starts_with([' ', '\t']) {
            continue;
        }
        in_tags = false;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "title" => front.title = scalar(value),
            "tags" | "tag" => {
                // A key with nothing after it opens a block sequence; anything
                // else is the whole list, on this line.
                if value.is_empty() {
                    in_tags = true;
                } else {
                    for tag in split_tags(value) {
                        push_tag(&mut front.tags, tag);
                    }
                }
            }
            // `created` is the one a vault writes deliberately, so it is not
            // displaced by a `date` further down the block.
            "created" | "date" => front.created = front.created.or(date_at(value, offset)),
            _ => {}
        }
    }
    front
}

/// The value of a `- item` line, at any indentation.
fn sequence_item(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('-')?;
    if rest.is_empty() {
        return Some("");
    }
    rest.strip_prefix(' ').map(str::trim)
}

/// A scalar value: unquoted and trimmed, `None` when nothing is left of it.
fn scalar(value: &str) -> Option<String> {
    let value = unquote(value.trim());
    (!value.is_empty()).then(|| value.to_string())
}

/// Drop one matching pair of surrounding quotes.
fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Split a one-line tag list. `[a, b]`, `a, b` and `a b` are all written in the
/// wild, and none of them can hold a tag with a space in it.
fn split_tags(value: &str) -> impl Iterator<Item = &str> {
    let inner = value
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(value);
    inner.split([',', ' ', '\t'])
}

/// Add `tag` to `tags` unless it is empty or already there. The leading `#` an
/// Obsidian tag is often written with is not part of the tag.
fn push_tag(tags: &mut Vec<String>, tag: &str) {
    let Some(tag) = scalar(tag) else { return };
    let tag = tag.trim_start_matches('#').to_string();
    if !tag.is_empty() && !tags.contains(&tag) {
        tags.push(tag);
    }
}

/// A front matter date as local midnight on the day it names.
///
/// Whatever follows the date — a time, a zone — is dropped: the column shows a
/// day, and a note that writes `09:00` has not said which zone that was.
fn date_at(value: &str, offset: time::UtcOffset) -> Option<i64> {
    let date = unquote(value.trim()).split(['T', ' ']).next()?;
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.trim().parse().ok()?;
    let month: u8 = parts.next()?.trim().parse().ok()?;
    let day: u8 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let month = time::Month::try_from(month).ok()?;
    Some(
        time::Date::from_calendar_date(year, month, day)
            .ok()?
            .midnight()
            .assume_offset(offset)
            .unix_timestamp(),
    )
}

// --- dates ------------------------------------------------------------------

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const YEAR: i64 = 365 * DAY;
const MONTH: i64 = YEAR / 12;

/// How long ago `then` was, in three or four characters — `now`, `42m`, `6d`,
/// `11mo`, `3y`.
///
/// The selector dates a note this way rather than with the calendar date the
/// listing prints: two calendar columns are twenty-two characters of a pane
/// already sharing its width with a preview, and the question a note list
/// answers is "which of these did I touch recently", not "on which Tuesday".
///
/// A note dated in the future — a clock that went backwards, a front matter
/// date typed ahead — reads as `now` rather than as a negative age.
pub fn age(now: i64, then: i64) -> String {
    let seconds = (now - then).max(0);
    match seconds {
        s if s < MINUTE => "now".to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < WEEK => format!("{}d", s / DAY),
        s if s < 5 * WEEK => format!("{}w", s / WEEK),
        // Never `0mo`: the arm above stops five weeks short of the average
        // month this divides by.
        s if s < YEAR => format!("{}mo", (s / MONTH).max(1)),
        s => format!("{}y", s / YEAR),
    }
}

/// A calendar date, `YYYY-MM-DD`, in local time.
pub fn date(when: i64, offset: time::UtcOffset) -> String {
    match local(when, offset) {
        Some(dt) => format!("{:04}-{:02}-{:02}", dt.year(), dt.month() as u8, dt.day()),
        None => BLANK_DATE.to_string(),
    }
}

/// A calendar date and the time of day, `YYYY-MM-DD HH:MM`, in local time.
pub fn timestamp(when: i64, offset: time::UtcOffset) -> String {
    match local(when, offset) {
        Some(dt) => format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            dt.year(),
            dt.month() as u8,
            dt.day(),
            dt.hour(),
            dt.minute(),
        ),
        None => BLANK_TIMESTAMP.to_string(),
    }
}

/// [`date`]- and [`timestamp`]-shaped blanks, so a time no calendar can
/// represent costs the listing a value rather than a column.
const BLANK_DATE: &str = "          ";
const BLANK_TIMESTAMP: &str = "                ";

fn local(when: i64, offset: time::UtcOffset) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::from_unix_timestamp(when)
        .ok()
        .map(|dt| dt.to_offset(offset))
}

// --- rows -------------------------------------------------------------------

/// The width of every column a note listing shares, measured over the whole
/// list so the columns line up.
///
/// Counted in characters, not bytes: `{:<width$}` pads by character count, so a
/// byte length would over-pad a title with an accent in it. A column measuring
/// zero is one nothing in this vault fills, and is left out of the row rather
/// than padded — a vault with no labels and no tags spends its width on names.
#[derive(Debug, Clone, Copy, Default)]
pub struct Widths {
    pub title: usize,
    pub group: usize,
    pub folder: usize,
    pub tags: usize,
    pub tasks: usize,
    pub modified: usize,
    pub created: usize,
    pub rel: usize,
}

impl Widths {
    pub fn of(notes: &[Note], cfg: &crate::config::NoteConfig, now: i64) -> Self {
        let widest = |f: &dyn Fn(&Note) -> usize| notes.iter().map(f).max().unwrap_or(0);
        Self {
            title: widest(&|n| n.title().chars().count()),
            group: widest(&|n| n.group(cfg).chars().count()),
            folder: widest(&|n| n.folder(cfg).chars().count()),
            tags: widest(&|n| n.tag_column().chars().count()),
            tasks: widest(&|n| n.tasks.column().chars().count()),
            modified: widest(&|n| age(now, n.modified).chars().count()),
            created: widest(&|n| age(now, n.created).chars().count()),
            rel: widest(&|n| n.rel.chars().count()),
        }
    }
}

/// One selector row: what the note calls itself first, then what is true of it.
///
/// The name leads because it is what is being looked for and what the query
/// matches; everything after it is an attribute, in a column of its own, in a
/// colour that says which attribute it is — the group cyan or green where it is
/// a configured label and grey where it is a bare directory name, the folder
/// blue as a path is everywhere, the tags magenta.
///
/// The row and its colours are built together because a tint is a character
/// range into the row, and counting those ranges out a second time is how they
/// drift.
///
/// Not padded to the full width: what follows on the row is
/// [`suffix`], and a row's trailing columns are only worth aligning when
/// something is drawn in them.
pub fn row(note: &Note, cfg: &crate::config::NoteConfig, widths: &Widths) -> (String, Vec<Tint>) {
    let mut row = String::new();
    let mut tints = Vec::new();
    let mut column = |row: &mut String, text: &str, width: usize, color: Option<u8>| {
        if width == 0 {
            return;
        }
        if !row.is_empty() {
            row.push_str(COLUMN_GAP);
        }
        let at = row.chars().count();
        push_column(row, text, width);
        if let Some(color) = color.filter(|_| !text.is_empty()) {
            tints.push(Tint {
                range: at..at + text.chars().count(),
                color,
            });
        }
    };

    column(&mut row, note.title(), widths.title, None);
    // A bare directory name is grey and a label is its own colour, so the
    // column says whether it was configured as well as what it holds.
    let group_color = note
        .labelled(cfg)
        .then(|| cfg.color_of(note.group(cfg)))
        .flatten()
        .or(Some(UNLABELLED_COLOR));
    column(&mut row, note.group(cfg), widths.group, group_color);
    column(
        &mut row,
        note.folder(cfg),
        widths.folder,
        Some(FOLDER_COLOR),
    );
    column(&mut row, &note.tag_column(), widths.tags, Some(TAG_COLOR));

    // Deliberately not trimmed: [`suffix`] is drawn straight after this, and a
    // row whose columns were trimmed away would start its dates wherever its
    // own text happened to stop.
    (row, tints)
}

/// The columns drawn after the label and not matched against: how much of the
/// note is still owed, then how long ago it was modified and created.
///
/// Behind the label rather than ahead of it, because the name is what a note
/// list is read down. Outside the label because none of it is what anybody
/// searches for — matched, a query of `3` would rank every note that is three
/// days old above the one being looked for.
///
/// Every row's is the same width, so the columns line up however narrow the
/// name column turned out.
pub fn suffix(note: &Note, now: i64, widths: &Widths) -> (String, Vec<Tint>) {
    let mut suffix = String::new();
    let mut tints = Vec::new();

    if widths.tasks > 0 {
        suffix.push_str(COLUMN_GAP);
        let tasks = note.tasks.column();
        let at = suffix.chars().count() + widths.tasks - tasks.chars().count();
        // Right-aligned: the glyph is the fixed part and the count grows to
        // the left of it.
        for _ in tasks.chars().count()..widths.tasks {
            suffix.push(' ');
        }
        suffix.push_str(&tasks);
        if !tasks.is_empty() {
            tints.push(Tint {
                range: at..suffix.chars().count(),
                color: note.tasks.color(),
            });
        }
    }

    suffix.push_str(COLUMN_GAP);
    push_right(&mut suffix, &age(now, note.modified), widths.modified);
    suffix.push(' ');
    push_right(&mut suffix, &age(now, note.created), widths.created);

    (suffix, tints)
}

/// Between two columns. Two spaces, everywhere, so a row reads as columns
/// rather than as a sentence.
const COLUMN_GAP: &str = "  ";

/// The colour of a group column holding a bare directory name: the terminal's
/// own grey, as [`crate::select::SelectItem::prefix`] uses, since an unlabelled
/// directory is context rather than a statement.
const UNLABELLED_COLOR: u8 = 8;

/// One `note ls --status` row: the note's name, its group, its tags, how many
/// tasks are open, and both dates.
///
/// The path comes first, so the plain listing is a prefix of this one. Modified
/// carries a time of day and created does not: a created date may have come
/// from front matter, which names a day.
pub fn status_row(
    note: &Note,
    cfg: &crate::config::NoteConfig,
    widths: &Widths,
    offset: time::UtcOffset,
) -> String {
    let mut row = String::new();
    push_column(&mut row, &note.rel, widths.rel);
    for (text, width) in [
        (note.group(cfg).to_string(), widths.group),
        (note.tag_column(), widths.tags),
        (note.tasks.column(), widths.tasks),
    ] {
        if width > 0 {
            row.push_str(COLUMN_GAP);
            push_column(&mut row, &text, width);
        }
    }
    row.push_str(COLUMN_GAP);
    row.push_str(&timestamp(note.modified, offset));
    row.push_str(COLUMN_GAP);
    row.push_str(&date(note.created, offset));
    row.trim_end().to_string()
}

/// Append `text` and pad it out to `width` characters.
fn push_column(row: &mut String, text: &str, width: usize) {
    row.push_str(text);
    for _ in text.chars().count()..width {
        row.push(' ');
    }
}

/// Append `text` padded to `width` characters on its left.
fn push_right(row: &mut String, text: &str, width: usize) {
    for _ in text.chars().count()..width {
        row.push(' ');
    }
    row.push_str(text);
}

/// Order a vault: most recently modified first, then by path so notes written
/// in the same second do not shuffle between runs.
///
/// Deliberately not [`crate::Ctx::by_recency`], which every other selector is
/// ordered by: opening a note is what changes its modification time, so the
/// file already records what that store would, and two orderings of one fact
/// can only disagree.
pub fn newest_first(notes: &mut [Note]) {
    notes.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.rel.cmp(&b.rel)));
}

/// Whether a path is one a note listing offers, by extension.
pub fn is_note(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            NOTE_EXTENSIONS
                .iter()
                .any(|kind| ext.eq_ignore_ascii_case(kind))
        })
}

/// What counts as a note. Obsidian writes Markdown and reads nothing else, and
/// a vault's other files — its attachments, its `.obsidian` settings — are not
/// things to hand an editor.
const NOTE_EXTENSIONS: &[&str] = &["md", "markdown"];

// --- preview ----------------------------------------------------------------

/// Colours the preview pane draws with. Grey for everything that is context
/// rather than content, cyan for headings, blue for a link, green for a task
/// that is done.
const DIM: u8 = 8;
const HEADING_COLOR: u8 = 6;
const LINK_COLOR: u8 = 4;
const DONE_COLOR: u8 = 2;

/// How much of a note the preview pane shows, matching what `bat` is asked for
/// elsewhere. A pane is thirty rows; anything past this is for the editor.
const PREVIEW_LINES: usize = 200;

/// The preview pane for a note: what the row could not fit, then the note.
///
/// The header is where the two age columns on the row are spelled out — a
/// column reading `3d  2y` says which is which only once something says it,
/// and the pane has the width to.
///
/// The body is drawn rather than shelled out to `bat`: a vault is thousands of
/// notes and skim runs a preview on every move through the list, so the pane
/// that costs nothing to produce is the one that keeps up. Front matter is not
/// repeated — the header above is what it says, read.
///
/// Rendering is line-level. Headings, quotes, list markers, task boxes and code
/// fences are what a note is made of at a glance; `**bold**` and its friends
/// are left as the author wrote them, since a pane that hides the markup it
/// cannot render is lying about the file.
pub fn preview(
    note: &Note,
    cfg: &crate::config::NoteConfig,
    text: &str,
    now: i64,
    offset: time::UtcOffset,
) -> String {
    let mut out = String::new();

    out.push_str(&bold(&crate::term::one_row(note.title())));
    out.push('\n');
    out.push_str(&paint(&crate::term::one_row(&note.rel), DIM));
    out.push('\n');

    let group = note.group(cfg);
    let mut facts = Vec::new();
    if !group.is_empty() {
        let color = match note.labelled(cfg) {
            true => cfg.color_of(group).unwrap_or(UNLABELLED_COLOR),
            false => UNLABELLED_COLOR,
        };
        facts.push(paint(&crate::term::one_row(group), color));
    }
    let tags = note.tag_column();
    if !tags.is_empty() {
        facts.push(paint(&crate::term::one_row(&tags), TAG_COLOR));
    }
    // Spelled out rather than left as the row's glyph and a number, which is
    // the pane's job: it has the width the column did not.
    if note.tasks.total() > 0 {
        facts.push(paint(
            &format!("{} of {} done", note.tasks.done, note.tasks.total()),
            note.tasks.color(),
        ));
    }
    if !facts.is_empty() {
        out.push_str(&facts.join(&paint("  \u{b7}  ", DIM)));
        out.push('\n');
    }

    out.push_str(&paint(
        &format!(
            "modified {} ({})   created {} ({})",
            timestamp(note.modified, offset),
            age(now, note.modified),
            date(note.created, offset),
            age(now, note.created),
        ),
        DIM,
    ));
    out.push('\n');

    let (_, body) = split_front_matter(text);
    let mut fenced = false;
    for line in body
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .take(PREVIEW_LINES)
    {
        // Every line reaching the terminal is a note's own bytes, so its
        // control characters go before anything is drawn: a note holding an
        // escape sequence would otherwise repaint the pane around it.
        out.push('\n');
        out.push_str(&markdown_line(&crate::term::one_row(line), &mut fenced));
    }
    out
}

/// One body line, drawn. `fenced` carries whether the line before it was inside
/// a fenced code block, which is the only state a line-level renderer needs.
fn markdown_line(line: &str, fenced: &mut bool) -> String {
    let trimmed = line.trim_start();

    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        *fenced = !*fenced;
        return paint(line, DIM);
    }
    // Inside a fence every character is the author's, `#` and `[[` included.
    if *fenced {
        return line.to_string();
    }

    if let Some(heading) = heading(trimmed) {
        return format!("\x1b[1;38;5;{HEADING_COLOR}m{heading}\x1b[0m");
    }
    if trimmed.starts_with('>') || is_rule(trimmed) {
        return paint(line, DIM);
    }
    match task_or_bullet(line) {
        Some((marker, rest)) => format!("{marker}{}", inline(rest)),
        None => inline(line),
    }
}

/// An ATX heading's text, `#` marks and all — kept, because a level is
/// something a reader counts.
fn heading(trimmed: &str) -> Option<&str> {
    let hashes = trimmed.len() - trimmed.trim_start_matches('#').len();
    ((1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ')).then_some(trimmed)
}

/// Whether the line is a thematic break rather than text.
fn is_rule(trimmed: &str) -> bool {
    let mark = trimmed.chars().next().filter(|c| "-*_".contains(*c));
    mark.is_some_and(|mark| {
        trimmed.chars().filter(|c| *c == mark).count() >= 3
            && trimmed.chars().all(|c| c == mark || c == ' ')
    })
}

/// A list line split into its drawn marker and the text after it: the bullet
/// dim, a task's box green once it is ticked.
fn task_or_bullet(line: &str) -> Option<(String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let (bullet, after) = bullet(&line[indent..])?;
    let head = &line[..indent + bullet.len()];

    let Some(box_end) = task_box(after) else {
        return Some((paint(head, DIM), after));
    };
    let (ticked, rest) = after.split_at(box_end);
    let done = !ticked.contains(|c: char| c == ' ' && ticked.starts_with("[ "));
    Some((
        format!(
            "{}{}",
            paint(head, DIM),
            paint(ticked, if done { DONE_COLOR } else { DIM })
        ),
        rest,
    ))
}

/// The `- `, `* `, `+ ` or `1. ` opening a list item, and what follows it.
fn bullet(text: &str) -> Option<(&str, &str)> {
    if let Some(rest) = text
        .strip_prefix(['-', '*', '+'])
        .and_then(|r| r.strip_prefix(' '))
    {
        return Some((&text[..2], rest));
    }
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = text[digits..].strip_prefix(['.', ')'])?.strip_prefix(' ')?;
    Some((&text[..digits + 2], rest))
}

/// Where a task checkbox ends, when the text opens with one.
fn task_box(text: &str) -> Option<usize> {
    let inner = text.strip_prefix('[')?;
    let mut chars = inner.chars();
    let mark = chars.next()?;
    chars.next().filter(|c| *c == ']')?;
    // `[ ]` and `[x]` and nothing wider: `[2024-01-01]` opens a link, not a
    // task.
    (mark == ' ' || !mark.is_whitespace()).then_some(3)
}

/// Colour what a note's body says about itself: `[[wikilinks]]` and `#tags`.
fn inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut at = 0;
    // A tag opens a word; `C#` in a sentence and a `#fragment` on a URL do not.
    let mut boundary = true;

    while at < line.len() {
        if line[at..].starts_with("[[")
            && let Some(end) = line[at + 2..].find("]]")
        {
            let link = &line[at..at + 2 + end + 2];
            out.push_str(&paint(link, LINK_COLOR));
            at += link.len();
            boundary = false;
            continue;
        }
        if boundary && let Some(tag) = tag_at(&line[at..]) {
            out.push_str(&paint(tag, TAG_COLOR));
            at += tag.len();
            boundary = false;
            continue;
        }
        let ch = line[at..].chars().next().expect("scanning by character");
        boundary = ch.is_whitespace();
        out.push(ch);
        at += ch.len_utf8();
    }
    out
}

/// The tag `text` opens with, if it opens with one. A tag is `#` and at least
/// one character of `[A-Za-z0-9/_-]`, not all of them digits — Obsidian's own
/// rule, and what keeps `#1234` an issue number and `#fff` a colour.
fn tag_at(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('#')?;
    let len = rest
        .find(|c: char| !(c.is_alphanumeric() || "/_-".contains(c)))
        .unwrap_or(rest.len());
    let tag = &rest[..len];
    (!tag.is_empty() && !tag.chars().all(|c| c.is_ascii_digit())).then(|| &text[..len + 1])
}

fn paint(text: &str, color: u8) -> String {
    crate::term::paint(text, color, true)
}

fn bold(text: &str) -> String {
    format!("\x1b[1m{text}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc() -> time::UtcOffset {
        time::UtcOffset::UTC
    }

    fn note(rel: &str, front: Front) -> Note {
        Note {
            path: PathBuf::from("/vault").join(rel),
            dir: top_dir(rel).to_string(),
            rel: rel.to_string(),
            modified: 0,
            created: 0,
            front,
            tasks: Tasks::default(),
        }
    }

    /// A vault where `work` is labelled and `scratch` is not.
    fn config() -> crate::config::NoteConfig {
        let mut labels = crate::config::Labels::new();
        labels.insert("work".to_string(), vec!["work".to_string()]);
        crate::config::NoteConfig {
            root: Some("/vault".into()),
            labels,
            editor: None,
        }
    }

    fn front(text: &str) -> Front {
        let (block, _) = split_front_matter(text);
        parse_front(block.expect("front matter"), utc())
    }

    // --- front matter ---

    #[test]
    fn a_block_is_what_sits_between_the_two_fences() {
        let (block, body) = split_front_matter("---\ntitle: A\n---\n# Heading\n");
        assert_eq!(block, Some("title: A\n"));
        assert_eq!(body, "# Heading\n");
    }

    /// A note that opens with a horizontal rule has no front matter, and must
    /// not lose its body to a block that never closes.
    #[test]
    fn an_unterminated_block_is_not_front_matter() {
        let text = "---\njust a rule, then prose\n";
        assert_eq!(split_front_matter(text), (None, text));
    }

    #[test]
    fn a_note_with_no_block_keeps_its_whole_body() {
        let text = "# Heading\n\nbody\n";
        assert_eq!(split_front_matter(text), (None, text));
    }

    #[test]
    fn a_block_may_close_with_the_other_yaml_terminator() {
        let (block, body) = split_front_matter("---\ntitle: A\n...\nbody\n");
        assert_eq!(block, Some("title: A\n"));
        assert_eq!(body, "body\n");
    }

    #[test]
    fn tags_are_read_from_a_flow_sequence_a_block_sequence_or_a_bare_list() {
        for written in [
            "tags: [rust, til]",
            "tags:\n  - rust\n  - til",
            "tags:\n- rust\n- til",
            "tags: rust til",
            "tags: rust, til",
            "tags: [\"rust\", 'til']",
            "tag: #rust #til",
        ] {
            let parsed = front(&format!("---\n{written}\n---\n"));
            assert_eq!(parsed.tags, vec!["rust", "til"], "{written:?}");
        }
    }

    /// The tag column is drawn with a `#`, so a vault that writes them that way
    /// must not end up with two.
    #[test]
    fn a_tag_is_stored_without_the_hash_it_is_drawn_with() {
        let parsed = front("---\ntags: [\"#rust\"]\n---\n");
        assert_eq!(parsed.tags, vec!["rust"]);
        assert_eq!(note("a.md", parsed).tag_column(), "#rust");
    }

    #[test]
    fn a_repeated_tag_is_listed_once() {
        assert_eq!(front("---\ntags: [a, b, a]\n---\n").tags, vec!["a", "b"]);
    }

    /// A block sequence belongs to the key that opened it. `aliases` is the one
    /// every vault has, and its items are not tags.
    #[test]
    fn a_sequence_under_another_key_is_not_read_as_tags() {
        let parsed = front("---\ntags:\n  - real\naliases:\n  - other\n  - names\n---\n");
        assert_eq!(parsed.tags, vec!["real"]);
    }

    #[test]
    fn a_nested_mapping_is_skipped_rather_than_read_as_keys() {
        let parsed = front("---\nobsidian:\n  title: nested\ntitle: real\n---\n");
        assert_eq!(parsed.title.as_deref(), Some("real"));
    }

    #[test]
    fn a_quoted_title_loses_its_quotes() {
        assert_eq!(
            front("---\ntitle: \"Error handling\"\n---\n")
                .title
                .as_deref(),
            Some("Error handling")
        );
    }

    /// A title with a colon in it is one value, not a key and a value: only the
    /// first colon separates them.
    #[test]
    fn a_title_may_contain_a_colon() {
        assert_eq!(
            front("---\ntitle: Rust: error handling\n---\n")
                .title
                .as_deref(),
            Some("Rust: error handling")
        );
    }

    #[test]
    fn a_created_date_becomes_local_midnight_on_the_day_it_names() {
        let offset = time::UtcOffset::from_hms(2, 0, 0).unwrap();
        let (block, _) = split_front_matter("---\ncreated: 2024-03-01\n---\n");
        let parsed = parse_front(block.unwrap(), offset);
        assert_eq!(date(parsed.created.unwrap(), offset), "2024-03-01");
    }

    #[test]
    fn a_created_value_carrying_a_time_still_names_a_day() {
        for written in ["2024-03-01T09:30:00", "2024-03-01 09:30", "\"2024-03-01\""] {
            let parsed = front(&format!("---\ncreated: {written}\n---\n"));
            assert_eq!(
                date(parsed.created.expect(written), utc()),
                "2024-03-01",
                "{written:?}"
            );
        }
    }

    #[test]
    fn a_created_value_that_is_not_a_date_is_no_date_at_all() {
        for written in ["someday", "2024", "not-a-date-x", "2024-13-01"] {
            let parsed = front(&format!("---\ncreated: {written}\n---\n"));
            assert_eq!(parsed.created, None, "{written:?}");
        }
    }

    #[test]
    fn a_vault_writing_date_rather_than_created_is_read_the_same_way() {
        let parsed = front("---\ndate: 2024-03-01\n---\n");
        assert_eq!(date(parsed.created.unwrap(), utc()), "2024-03-01");
    }

    // --- what a note calls itself ---

    #[test]
    fn a_note_falls_back_to_its_filename_and_takes_a_title_over_it() {
        assert_eq!(note("work/standup.md", Front::default()).title(), "standup");
        assert_eq!(
            note(
                "work/standup.md",
                Front {
                    title: Some("Daily standup".into()),
                    ..Front::default()
                }
            )
            .title(),
            "Daily standup"
        );
    }

    /// The folder is what sits between the group's directory and the note, so
    /// a row never writes the group twice.
    #[test]
    fn a_note_at_the_vault_root_is_filed_under_nothing() {
        let bare = crate::config::NoteConfig::default();
        assert_eq!(note("inbox.md", Front::default()).folder(&bare), "");
        assert_eq!(note("a/c.md", Front::default()).folder(&bare), "");
        assert_eq!(note("a/b/c.md", Front::default()).folder(&bare), "b");
        assert_eq!(note("a/b/c/d.md", Front::default()).folder(&bare), "b/c");
    }

    /// The filesystem dates a synced or freshly cloned vault the day it
    /// arrived, which is why front matter wins.
    #[test]
    fn a_creation_date_prefers_front_matter_then_the_filesystem_then_the_edit() {
        assert_eq!(created(Some(10), Some(20), 30), 10);
        assert_eq!(created(None, Some(20), 30), 20);
        assert_eq!(created(None, None, 30), 30);
    }

    // --- dates ---

    #[test]
    fn an_age_reads_as_the_largest_unit_that_fits() {
        let cases = [
            (0, "now"),
            (59, "now"),
            (60, "1m"),
            (59 * MINUTE, "59m"),
            (HOUR, "1h"),
            (DAY, "1d"),
            (6 * DAY, "6d"),
            (WEEK, "1w"),
            (4 * WEEK, "4w"),
            (5 * WEEK, "1mo"),
            (364 * DAY, "11mo"),
            (YEAR, "1y"),
        ];
        for (elapsed, want) in cases {
            assert_eq!(age(elapsed, 0), want, "{elapsed}s");
        }
    }

    /// A clock that went backwards, or a front matter date typed ahead, must
    /// not print a negative age into a right-aligned column.
    #[test]
    fn a_note_dated_in_the_future_reads_as_now() {
        assert_eq!(age(0, 10_000), "now");
    }

    #[test]
    fn an_age_never_rounds_down_to_no_months_at_all() {
        for elapsed in (5 * WEEK)..YEAR {
            let stamp = age(elapsed, 0);
            assert!(stamp.starts_with(|c: char| c != '0'), "{elapsed}s: {stamp}");
        }
    }

    // --- rows ---

    fn vault() -> Vec<Note> {
        vec![
            Note {
                modified: 3 * DAY,
                created: 400 * DAY,
                tasks: Tasks { open: 2, done: 5 },
                ..note(
                    "work/meetings/standup.md",
                    Front {
                        tags: vec!["daily".into()],
                        ..Front::default()
                    },
                )
            },
            Note {
                modified: 0,
                created: 0,
                ..note("scratch/idea.md", Front::default())
            },
            Note {
                modified: 0,
                created: 0,
                ..note("inbox.md", Front::default())
            },
        ]
    }

    fn rows(notes: &[Note]) -> Vec<String> {
        let cfg = config();
        let widths = Widths::of(notes, &cfg, 0);
        notes.iter().map(|n| row(n, &cfg, &widths).0).collect()
    }

    /// The name is what the eye runs down and what the query matches, so it
    /// leads; everything after it is an attribute in a column of its own.
    #[test]
    fn a_row_leads_with_the_note_name() {
        for row in rows(&vault()) {
            let first = row.split_whitespace().next().unwrap_or_default();
            assert!(
                ["standup", "idea", "inbox"].contains(&first),
                "a row does not open with its name: {row:?}"
            );
        }
    }

    /// A labelled directory shows its label; an unlabelled one shows itself,
    /// rather than the `-` a repository row would carry. A vault of five
    /// directories with two labelled would otherwise show three rows saying
    /// nothing.
    #[test]
    fn the_group_column_is_the_label_or_the_directory_that_has_none() {
        let cfg = config();
        let notes = vault();
        assert_eq!(notes[0].group(&cfg), "work");
        assert!(notes[0].labelled(&cfg));
        assert_eq!(notes[1].group(&cfg), "scratch");
        assert!(!notes[1].labelled(&cfg));
        assert_eq!(notes[2].group(&cfg), "");
    }

    /// Between them, the group and folder columns spell out a note's path
    /// exactly once: a label names no directory, so the whole path is still
    /// news; a bare directory name is already drawn, so only what is below it
    /// is.
    #[test]
    fn the_group_and_folder_columns_spell_the_path_out_once() {
        let cfg = config();
        let notes = vault();
        // `work` is a label, so the directory it labels is still worth drawing.
        assert_eq!(notes[0].folder(&cfg), "work/meetings");
        // `scratch` is the directory itself, one column to the left already.
        assert_eq!(notes[1].folder(&cfg), "");
        assert_eq!(notes[2].folder(&cfg), "");
    }

    /// Columns are the whole point: a name one character longer must not shift
    /// the group on every other row.
    #[test]
    fn the_columns_line_up_across_the_whole_list() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 0);
        for (row, note) in rows(&notes).iter().zip(&notes) {
            let group = note.group(&cfg);
            if group.is_empty() {
                continue;
            }
            let after_name: String = row.chars().skip(widths.title + COLUMN_GAP.len()).collect();
            assert!(
                after_name.starts_with(group),
                "the group column moved: {row:?}"
            );
        }
    }

    #[test]
    fn column_width_counts_characters_not_bytes() {
        let notes = vec![
            note("café.md", Front::default()),
            note("a.md", Front::default()),
        ];
        let widths = Widths::of(&notes, &config(), 0);
        assert_eq!(widths.title, "café".chars().count());
    }

    /// A column nothing in this vault fills is left out rather than padded, so
    /// a vault with no labels and no tags spends its width on names.
    #[test]
    fn an_empty_column_is_not_drawn_at_all() {
        let notes = vec![
            note("a.md", Front::default()),
            note("b.md", Front::default()),
        ];
        let cfg = crate::config::NoteConfig::default();
        let widths = Widths::of(&notes, &cfg, 0);
        assert_eq!(widths.group, 0);
        assert_eq!(widths.tags, 0);
        assert_eq!(row(&notes[0], &cfg, &widths).0, "a");
    }

    #[test]
    fn a_row_tints_each_attribute_and_leaves_the_name_alone() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 0);
        let (label, tints) = row(&notes[0], &cfg, &widths);
        let text = |tint: &Tint| -> String {
            label
                .chars()
                .skip(tint.range.start)
                .take(tint.range.len())
                .collect()
        };
        let drawn: Vec<(String, u8)> = tints.iter().map(|t| (text(t), t.color)).collect();
        assert_eq!(drawn[0].0, "work");
        assert_eq!(drawn[1], ("work/meetings".to_string(), FOLDER_COLOR));
        assert_eq!(drawn[2], ("#daily".to_string(), TAG_COLOR));
        assert!(
            !tints.iter().any(|t| t.range.start == 0),
            "the name is tinted, and skim's match highlight has to win: {drawn:?}"
        );
    }

    /// An unlabelled directory is context rather than a statement, so it is
    /// grey where a configured label takes its own hue.
    #[test]
    fn a_bare_directory_name_is_not_coloured_like_a_label() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 0);
        let color_of = |n: &Note| row(n, &cfg, &widths).1.first().map(|t| t.color);
        assert_eq!(color_of(&notes[1]), Some(UNLABELLED_COLOR));
        assert_ne!(color_of(&notes[0]), Some(UNLABELLED_COLOR));
    }

    /// The dates and the task count sit behind the name and outside what the
    /// query matches — typed, `3` must not rank every three-day-old note above
    /// the one being looked for.
    #[test]
    fn the_suffix_holds_what_nobody_searches_for() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 3 * DAY);
        let (label, _) = row(&notes[0], &cfg, &widths);
        let (suffix, _) = suffix(&notes[0], 3 * DAY, &widths);
        assert!(suffix.contains("now"), "{suffix:?}");
        assert!(suffix.contains(OPEN_TASK), "{suffix:?}");
        assert!(!label.contains("now"), "{label:?}");
        assert!(!label.contains(OPEN_TASK), "{label:?}");
    }

    #[test]
    fn every_suffix_is_the_same_width() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 10 * YEAR);
        let widths_drawn: Vec<usize> = notes
            .iter()
            .map(|n| suffix(n, 10 * YEAR, &widths).0.chars().count())
            .collect();
        assert!(
            widths_drawn.windows(2).all(|w| w[0] == w[1]),
            "{widths_drawn:?}"
        );
    }

    #[test]
    fn a_note_with_tasks_left_reads_differently_from_one_without() {
        assert_eq!(Tasks { open: 0, done: 0 }.column(), "");
        assert_eq!(Tasks { open: 0, done: 3 }.column(), ALL_DONE);
        assert_eq!(Tasks { open: 2, done: 3 }.column(), format!("2{OPEN_TASK}"));
        assert_eq!(Tasks { open: 0, done: 3 }.color(), DONE_COLOR);
        assert_eq!(Tasks { open: 2, done: 3 }.color(), OPEN_COLOR);
    }

    #[test]
    fn tasks_are_counted_from_the_body() {
        let counted = count_tasks("- [ ] a\n* [x] b\n  + [X] c\n- not a task\n1. [ ] d\n");
        assert_eq!(counted, Tasks { open: 2, done: 2 });
    }

    /// A checkbox inside a fence is an example of one, not one.
    #[test]
    fn a_task_in_a_code_fence_is_not_counted() {
        let counted = count_tasks("- [ ] real\n```md\n- [ ] example\n```\n");
        assert_eq!(counted, Tasks { open: 1, done: 0 });
    }

    #[test]
    fn a_status_row_leads_with_the_path_and_ends_with_both_dates() {
        let cfg = config();
        let notes = vault();
        let widths = Widths::of(&notes, &cfg, 0);
        let row = status_row(&notes[0], &cfg, &widths, utc());
        assert!(row.starts_with("work/meetings/standup.md"), "{row:?}");
        assert!(row.contains("work"), "{row:?}");
        assert!(row.contains("#daily"), "{row:?}");
        assert!(row.ends_with("1971-02-05"), "{row:?}");
    }

    #[test]
    fn a_listing_leads_with_what_was_touched_most_recently() {
        let mut notes = vault();
        newest_first(&mut notes);
        assert_eq!(notes[0].rel, "work/meetings/standup.md");
    }

    #[test]
    fn only_markdown_is_a_note() {
        for name in ["a.md", "a.markdown", "a.MD"] {
            assert!(is_note(Path::new(name)), "{name}");
        }
        for name in ["a.png", "a.canvas", "a", "a.md.bak"] {
            assert!(!is_note(Path::new(name)), "{name}");
        }
    }

    // --- preview ---

    fn preview_of(text: &str) -> String {
        preview(
            &note("work/a.md", Front::default()),
            &config(),
            text,
            0,
            utc(),
        )
    }

    /// The header is where the row's two age columns are named; without it the
    /// pair reads as two numbers.
    #[test]
    fn the_preview_names_both_dates_the_row_only_shows() {
        let rendered = preview_of("body\n");
        assert!(rendered.contains("modified "), "{rendered}");
        assert!(rendered.contains("created "), "{rendered}");
    }

    /// The header already says everything the block does, spelled out.
    #[test]
    fn the_preview_shows_the_body_and_not_the_front_matter() {
        let rendered = preview_of("---\ntitle: A\ntags: [x]\n---\n\n# Heading\n");
        assert!(rendered.contains("Heading"), "{rendered}");
        assert!(!rendered.contains("tags: [x]"), "{rendered}");
    }

    /// A note is foreign text: an escape sequence in one would otherwise
    /// repaint the pane around it. Only scriv's own colours survive the body.
    #[test]
    fn a_note_cannot_colour_the_pane_with_its_own_escapes() {
        let rendered = preview_of("plain \x1b[31mred\x1b[0m\n");
        let body = rendered.split_once("\n\n").expect("a body").1;
        assert!(body.contains("red"), "{body:?}");
        assert!(!body.contains('\x1b'), "{body:?}");
    }

    #[test]
    fn a_preview_stops_at_the_line_it_is_bounded_to() {
        let long = "line\n".repeat(PREVIEW_LINES * 2);
        let rendered = preview_of(&long);
        assert_eq!(rendered.matches("line").count(), PREVIEW_LINES);
    }

    fn drawn(line: &str) -> String {
        markdown_line(line, &mut false)
    }

    #[test]
    fn a_heading_keeps_its_hashes_so_its_level_is_still_countable() {
        assert!(drawn("## Notes").contains("## Notes"));
    }

    #[test]
    fn a_ticked_task_and_an_open_one_are_not_drawn_alike() {
        assert_ne!(drawn("- [x] done"), drawn("- [ ] open"));
        assert!(drawn("- [x] done").contains("done"));
    }

    #[test]
    fn a_wikilink_and_a_tag_are_coloured_where_they_appear() {
        let rendered = drawn("see [[other note]] about #rust");
        assert!(
            rendered.contains(&paint("[[other note]]", LINK_COLOR)),
            "{rendered:?}"
        );
        assert!(
            rendered.contains(&paint("#rust", TAG_COLOR)),
            "{rendered:?}"
        );
    }

    /// A `#` that opens no word is not a tag: `#1234` is an issue number and
    /// `#fff` a colour, and both sit in the middle of prose.
    #[test]
    fn a_hash_that_is_not_a_tag_is_left_alone() {
        for line in ["issue #1234", "C# is a language", "url#fragment"] {
            assert_eq!(drawn(line), line, "{line:?}");
        }
    }

    /// Inside a fence every character is the author's, `#` and `[[` included.
    #[test]
    fn a_fenced_block_is_not_rendered_as_markdown() {
        let mut fenced = false;
        markdown_line("```sh", &mut fenced);
        assert!(fenced);
        assert_eq!(
            markdown_line("# not a heading", &mut fenced),
            "# not a heading"
        );
        markdown_line("```", &mut fenced);
        assert!(!fenced);
    }
}

// --- searching --------------------------------------------------------------

/// One line of a note that a search matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    /// Absolute path, which is what an editor and a quickfix entry both want.
    pub path: PathBuf,
    /// Path below the vault root, which is what a row shows.
    pub rel: String,
    pub line: u32,
    pub column: u32,
    /// The matched line, as ripgrep printed it.
    pub text: String,
}

/// Read one `--vimgrep` line: `path:line:column:text`.
///
/// Split from the left exactly three times, because only the first three fields
/// are known not to contain a colon — a path may, and the matched text very
/// often does.
pub fn parse_match(line: &str, root: &Path) -> Option<Match> {
    // The path is whatever precedes the last colon that still leaves two
    // numbers and a text behind it, which reading from the right finds without
    // guessing where the path ends.
    let (head, text) = split_three(line)?;
    let (path, line_no, column) = head;
    let path = PathBuf::from(path);
    Some(Match {
        rel: path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned(),
        path,
        line: line_no,
        column,
        text: text.to_string(),
    })
}

/// `path:line:column:` and the rest, where `line` and `column` are the last two
/// colon-separated numbers before the text.
fn split_three(line: &str) -> Option<((&str, u32, u32), &str)> {
    let mut at = 0;
    // Walk the colons left to right: the first one whose next two fields are
    // numbers ends the path. A path holding `x:1:2:` before the real fields is
    // a file nobody has.
    while let Some(next) = line[at..].find(':') {
        let end = at + next;
        let rest = &line[end + 1..];
        if let Some((line_no, rest)) = take_number(rest)
            && let Some((column, rest)) = take_number(rest)
        {
            return Some(((&line[..end], line_no, column), rest));
        }
        at = end + 1;
    }
    None
}

/// A run of digits followed by a colon, and what comes after it.
fn take_number(text: &str) -> Option<(u32, &str)> {
    let digits = text.len() - text.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits == 0 {
        return None;
    }
    let rest = text[digits..].strip_prefix(':')?;
    Some((text[..digits].parse().ok()?, rest))
}

/// The field separator inside a search row's value.
///
/// A control character rather than a colon or a tab: the value carries a path,
/// which may hold either, and it is read back by [`decode_match`] rather than
/// by a person.
const FIELD: char = '\u{1}';

/// Pack a match into the string a selector row returns.
pub fn encode_match(found: &Match) -> String {
    format!(
        "{}{FIELD}{}{FIELD}{}{FIELD}{}",
        found.path.display(),
        found.line,
        found.column,
        crate::term::one_row(&found.text),
    )
}

/// Read back what [`encode_match`] wrote.
pub fn decode_match(value: &str, root: &Path) -> Option<Match> {
    let mut fields = value.splitn(4, FIELD);
    let path = PathBuf::from(fields.next()?);
    let line = fields.next()?.parse().ok()?;
    let column = fields.next()?.parse().ok()?;
    let text = fields.next().unwrap_or_default().to_string();
    Some(Match {
        rel: path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned(),
        path,
        line,
        column,
        text,
    })
}

/// One entry of a quickfix list, in the `file:line:column:text` shape vim's
/// `errorformat` is told to expect.
pub fn quickfix_line(found: &Match) -> String {
    format!(
        "{}:{}:{}:{}",
        found.path.display(),
        found.line,
        found.column,
        crate::term::one_row(&found.text)
    )
}

/// The widest `rel:line` in a batch of matches, so their text columns line up.
pub fn match_width(matches: &[Match]) -> usize {
    matches
        .iter()
        .map(|m| m.rel.chars().count() + 1 + m.line.to_string().chars().count())
        .max()
        .unwrap_or(0)
}

/// One search row: where the match is, then the line that matched.
///
/// The location leads for the reason a note's name does — it is what the eye
/// runs down — and is the only part coloured, since the matched text is the
/// note's own words and colouring those would be scriv talking over them.
pub fn match_row(found: &Match, width: usize) -> (String, Vec<Tint>) {
    let location = format!("{}:{}", found.rel, found.line);
    let mut row = String::new();
    push_column(&mut row, &location, width);
    row.push_str(COLUMN_GAP);
    // Trimmed at the front only: the indentation of a matched line is noise in
    // a list, and its trailing text is what the match is.
    row.push_str(crate::term::one_row(found.text.trim_start()).trim_end());

    let at = found.rel.chars().count();
    (
        row,
        vec![
            Tint {
                range: 0..at,
                color: FOLDER_COLOR,
            },
            Tint {
                range: at..location.chars().count(),
                color: LINE_COLOR,
            },
        ],
    )
}

/// The colour of a `:line` suffix on a search row: yellow, as every grep-alike
/// numbers its lines.
const LINE_COLOR: u8 = 3;

/// How much of a note the search preview shows either side of the match.
const MATCH_CONTEXT: usize = 12;

/// The preview pane for a search row: the matched line in the note around it,
/// with the match itself marked.
pub fn match_preview(found: &Match, text: &str) -> String {
    let mut out = paint(&crate::term::one_row(&found.rel), DIM);
    out.push('\n');

    let line = found.line.max(1) as usize;
    let first = line.saturating_sub(MATCH_CONTEXT).max(1);
    let number_width = (line + MATCH_CONTEXT).to_string().len();

    for (offset, body) in text
        .lines()
        .enumerate()
        .skip(first - 1)
        .take(MATCH_CONTEXT * 2 + 1)
    {
        let number = offset + 1;
        let body = crate::term::one_row(body);
        out.push('\n');
        out.push_str(&paint(&format!("{number:>number_width$}"), DIM));
        out.push(' ');
        if number == line {
            out.push_str(&format!("\x1b[1;38;5;{LINE_COLOR}m{body}\x1b[0m"));
        } else {
            out.push_str(&body);
        }
    }
    out
}

// --- new notes --------------------------------------------------------------

/// The name a new note takes when the user did not give one: the local date and
/// time, to the minute.
///
/// A name rather than a prompt, because being asked to name a note is being
/// asked what it is about before writing it — and a note that has to be named
/// first is one that does not get written. It sorts, it is unique to the
/// minute, and renaming it afterwards is what an editor is for.
pub fn generated_name(now: i64, offset: time::UtcOffset) -> String {
    let Some(dt) = local(now, offset) else {
        return "note.md".to_string();
    };
    format!(
        "{:04}-{:02}-{:02}-{:02}{:02}.md",
        dt.year(),
        dt.month() as u8,
        dt.day(),
        dt.hour(),
        dt.minute(),
    )
}

/// A name nothing is using yet: `name`, else `name-2`, `name-3` and so on.
///
/// Two notes started in the same minute is not an error, and neither is one
/// name typed twice — the second is a second note.
pub fn free_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(name) {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    // Bounded only by the filesystem: a caller that finds every name taken has
    // a directory problem rather than a naming one.
    (2..)
        .map(|n| format!("{stem}-{n}{ext}"))
        .find(|candidate| !taken(candidate))
        .expect("the integers run out before the filenames do")
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/vault")
    }

    #[test]
    fn a_vimgrep_line_splits_into_a_place_and_the_text_that_matched() {
        let found = parse_match("/vault/work/a.md:12:5:the matched text", &root()).unwrap();
        assert_eq!(found.rel, "work/a.md");
        assert_eq!((found.line, found.column), (12, 5));
        assert_eq!(found.text, "the matched text");
    }

    /// The matched text is a line of prose, and prose is full of colons. Only
    /// the first three fields are known not to be.
    #[test]
    fn a_matched_line_may_contain_colons_of_its_own() {
        let found = parse_match("/vault/a.md:1:1:see also: the other note", &root()).unwrap();
        assert_eq!(found.text, "see also: the other note");
    }

    /// A path may contain a colon too, which is why the split looks for the
    /// two numbers rather than counting from the left.
    #[test]
    fn a_path_with_a_colon_in_it_still_parses() {
        let found = parse_match("/vault/a:b.md:7:2:text", &root()).unwrap();
        assert_eq!(found.path, PathBuf::from("/vault/a:b.md"));
        assert_eq!(found.line, 7);
    }

    #[test]
    fn a_line_that_is_not_a_match_is_not_one() {
        for line in ["", "no colons at all", "/vault/a.md:not:a:number"] {
            assert!(parse_match(line, &root()).is_none(), "{line:?}");
        }
    }

    /// The value travels through skim as one string and comes back to be
    /// opened, so what goes in has to come out — including a path holding the
    /// characters a simpler separator would have split on.
    #[test]
    fn a_match_survives_the_trip_through_the_selector() {
        let found = Match {
            path: PathBuf::from("/vault/a:b c.md"),
            rel: "a:b c.md".into(),
            line: 41,
            column: 3,
            text: "a line with: colons and\ttabs".into(),
        };
        let back = decode_match(&encode_match(&found), &root()).unwrap();
        assert_eq!(back.path, found.path);
        assert_eq!((back.line, back.column), (41, 3));
        assert_eq!(back.rel, "a:b c.md");
    }

    /// vim reads the list with `errorformat=%f:%l:%c:%m`, so this is the one
    /// shape it parses.
    #[test]
    fn a_quickfix_entry_is_file_line_column_text() {
        let found = parse_match("/vault/a.md:12:5:text", &root()).unwrap();
        assert_eq!(quickfix_line(&found), "/vault/a.md:12:5:text");
    }

    /// A note's own bytes reach the terminal here; an escape sequence in one
    /// would otherwise repaint the list around it.
    #[test]
    fn a_matched_line_cannot_colour_the_row_it_is_drawn_in() {
        let found = parse_match("/vault/a.md:1:1:red \x1b[31mhere", &root()).unwrap();
        let (row, _) = match_row(&found, 10);
        assert!(!row.contains('\x1b'), "{row:?}");
        assert!(!quickfix_line(&found).contains('\x1b'));
    }

    #[test]
    fn a_search_row_colours_where_the_match_is_and_not_what_it_says() {
        let found = parse_match("/vault/work/a.md:12:1:some words", &root()).unwrap();
        let (row, tints) = match_row(&found, 20);
        let text = |tint: &Tint| -> String {
            row.chars()
                .skip(tint.range.start)
                .take(tint.range.len())
                .collect()
        };
        assert_eq!(text(&tints[0]), "work/a.md");
        assert_eq!(text(&tints[1]), ":12");
        assert!(row.contains("some words"));
        assert!(
            tints
                .iter()
                .all(|t| t.range.end <= "work/a.md:12".chars().count()),
            "the matched text is coloured over"
        );
    }

    // --- new notes ---

    /// Sortable, and unique to the minute — which is what lets a note be
    /// started without first being named.
    #[test]
    fn a_generated_name_is_the_date_and_time() {
        assert_eq!(generated_name(0, utc()), "1970-01-01-0000.md");
        assert_eq!(
            generated_name(3 * 3600 + 25 * 60, utc()),
            "1970-01-01-0325.md"
        );
    }

    #[test]
    fn a_name_already_taken_takes_the_next_one() {
        let taken = |name: &str| ["a.md", "a-2.md"].contains(&name);
        assert_eq!(free_name("a.md", taken), "a-3.md");
        assert_eq!(free_name("b.md", taken), "b.md");
    }

    #[test]
    fn a_name_with_no_extension_is_still_given_a_free_one() {
        assert_eq!(free_name("a", |name| name == "a"), "a-2");
    }

    fn utc() -> time::UtcOffset {
        time::UtcOffset::UTC
    }
}
