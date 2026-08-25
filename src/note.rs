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
    /// Last modified, in Unix seconds.
    pub modified: i64,
    /// Created, in Unix seconds. See [`created`].
    pub created: i64,
    pub front: Front,
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

    /// The directory the note sits in, below the vault root — empty for a note
    /// at the root itself.
    pub fn folder(&self) -> &str {
        match self.rel.rfind('/') {
            Some(at) => &self.rel[..at],
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
/// byte length would over-pad a title with an accent in it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Widths {
    pub modified: usize,
    pub created: usize,
    pub title: usize,
    pub folder: usize,
    pub rel: usize,
    pub tags: usize,
}

impl Widths {
    pub fn of(notes: &[Note], now: i64) -> Self {
        let widest = |f: &dyn Fn(&Note) -> usize| notes.iter().map(f).max().unwrap_or(0);
        Self {
            modified: widest(&|n| age(now, n.modified).chars().count()),
            created: widest(&|n| age(now, n.created).chars().count()),
            title: widest(&|n| n.title().chars().count()),
            folder: widest(&|n| n.folder().chars().count()),
            rel: widest(&|n| n.rel.chars().count()),
            tags: widest(&|n| n.tag_column().chars().count()),
        }
    }
}

/// The dim column ahead of a selector row: how long ago the note was modified,
/// then how long ago it was created.
///
/// A [`crate::select::SelectItem::prefix`] rather than part of the label, so it
/// is shown without being searched — matched, a query of `3` would rank every
/// note that is three days old above the one being looked for. Modified comes
/// first because it is the order the list is in.
pub fn prefix(note: &Note, now: i64, widths: &Widths) -> String {
    format!(
        "{modified:>mw$} {created:>cw$}  ",
        modified = age(now, note.modified),
        mw = widths.modified,
        created = age(now, note.created),
        cw = widths.created,
    )
}

/// One selector row: what the note calls itself, the folder it is filed under,
/// and its tags.
///
/// The row and its colours are built together because a tint is a character
/// range into the row, and counting those ranges out a second time is how they
/// drift.
pub fn row(note: &Note, widths: &Widths) -> (String, Vec<Tint>) {
    let mut row = String::new();
    let mut tints = Vec::new();

    push_column(&mut row, note.title(), widths.title);

    if widths.folder > 0 {
        row.push_str("  ");
        let at = row.chars().count();
        push_column(&mut row, note.folder(), widths.folder);
        tints.push(Tint {
            range: at..at + note.folder().chars().count(),
            color: FOLDER_COLOR,
        });
    }

    let tags = note.tag_column();
    if !tags.is_empty() {
        row.push_str("  ");
        let at = row.chars().count();
        row.push_str(&tags);
        tints.push(Tint {
            range: at..row.chars().count(),
            color: TAG_COLOR,
        });
    }

    (row.trim_end().to_string(), tints)
}

/// One `note ls --status` row: the note's name, its tags, and both dates.
///
/// The name comes first, so the plain listing is a prefix of this one. Modified
/// carries a time of day and created does not: a created date may have come
/// from front matter, which names a day.
pub fn status_row(note: &Note, widths: &Widths, offset: time::UtcOffset) -> String {
    let mut row = String::new();
    push_column(&mut row, &note.rel, widths.rel);
    if widths.tags > 0 {
        row.push_str("  ");
        push_column(&mut row, &note.tag_column(), widths.tags);
    }
    row.push_str("  ");
    row.push_str(&timestamp(note.modified, offset));
    row.push_str("  ");
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
pub fn preview(note: &Note, text: &str, now: i64, offset: time::UtcOffset) -> String {
    let mut out = String::new();

    out.push_str(&bold(&crate::term::one_row(note.title())));
    out.push('\n');
    out.push_str(&paint(&crate::term::one_row(&note.rel), DIM));
    out.push('\n');

    let tags = note.tag_column();
    if !tags.is_empty() {
        out.push_str(&paint(&crate::term::one_row(&tags), TAG_COLOR));
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
            rel: rel.to_string(),
            modified: 0,
            created: 0,
            front,
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

    #[test]
    fn a_note_at_the_vault_root_is_filed_under_nothing() {
        assert_eq!(note("inbox.md", Front::default()).folder(), "");
        assert_eq!(note("a/b/c.md", Front::default()).folder(), "a/b");
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
                created: 3 * DAY,
                ..note(
                    "work/meetings/standup.md",
                    Front {
                        tags: vec!["work".into()],
                        ..Front::default()
                    },
                )
            },
            Note {
                modified: 0,
                created: 0,
                ..note("inbox.md", Front::default())
            },
        ]
    }

    fn labels(notes: &[Note]) -> Vec<String> {
        let widths = Widths::of(notes, 0);
        notes.iter().map(|n| row(n, &widths).0).collect()
    }

    #[test]
    fn a_row_carries_the_title_the_folder_and_the_tags() {
        assert_eq!(
            labels(&vault())[0],
            "standup  work/meetings  #work",
            "{:?}",
            labels(&vault())
        );
    }

    #[test]
    fn a_row_without_tags_ends_at_its_folder() {
        assert_eq!(labels(&vault())[1], "inbox");
    }

    /// The columns are what make a listing scannable; a title one character
    /// longer must not shift the folder on every other row.
    #[test]
    fn the_columns_line_up_across_the_whole_list() {
        let notes = vault();
        let widths = Widths::of(&notes, 0);
        let at = |label: &str| {
            label
                .find("work/meetings")
                .or_else(|| label.find("  "))
                .unwrap()
        };
        let rows: Vec<String> = notes.iter().map(|n| row(n, &widths).0).collect();
        assert_eq!(
            rows[0].find("work/meetings"),
            Some(widths.title + 2),
            "{rows:?}"
        );
        assert!(at(&rows[0]) >= widths.title, "{rows:?}");
    }

    #[test]
    fn column_width_counts_characters_not_bytes() {
        let notes = vec![
            note("café.md", Front::default()),
            note("a.md", Front::default()),
        ];
        let widths = Widths::of(&notes, 0);
        assert_eq!(widths.title, "café".chars().count());
    }

    #[test]
    fn a_row_tints_its_folder_and_its_tags_and_leaves_the_title_alone() {
        let notes = vault();
        let widths = Widths::of(&notes, 0);
        let (label, tints) = row(&notes[0], &widths);
        let text = |tint: &Tint| -> String {
            label
                .chars()
                .skip(tint.range.start)
                .take(tint.range.len())
                .collect()
        };
        assert_eq!(tints.len(), 2, "{label:?}");
        assert_eq!(text(&tints[0]), "work/meetings");
        assert_eq!(tints[0].color, FOLDER_COLOR);
        assert_eq!(text(&tints[1]), "#work");
        assert_eq!(tints[1].color, TAG_COLOR);
    }

    /// The prefix is what holds the two age columns, and it is not matched
    /// against — a query of `3` must not rank every three-day-old note first.
    #[test]
    fn the_prefix_holds_both_ages_and_the_label_holds_neither() {
        let notes = vault();
        let widths = Widths::of(&notes, 3 * DAY);
        let (label, _) = row(&notes[0], &widths);
        assert_eq!(prefix(&notes[0], 3 * DAY, &widths), "now now  ");
        assert!(!label.contains("now"), "{label:?}");
    }

    #[test]
    fn every_prefix_is_the_same_width() {
        let notes = vault();
        let widths = Widths::of(&notes, 10 * YEAR);
        let rendered: Vec<usize> = notes
            .iter()
            .map(|n| prefix(n, 10 * YEAR, &widths).chars().count())
            .collect();
        assert!(rendered.windows(2).all(|w| w[0] == w[1]), "{rendered:?}");
    }

    #[test]
    fn a_status_row_leads_with_the_name_and_ends_with_both_dates() {
        let notes = vault();
        let widths = Widths::of(&notes, 0);
        let row = status_row(&notes[0], &widths, utc());
        assert!(row.starts_with("work/meetings/standup.md"), "{row:?}");
        assert!(row.contains("#work"), "{row:?}");
        assert!(row.ends_with("1970-01-04"), "{row:?}");
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
        preview(&note("work/a.md", Front::default()), text, 0, utc())
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
