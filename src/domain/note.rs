//! Domain: notes (`CONTEXT.md`, **Note**). A note is a remark the person attaches to a line, a
//! range of lines or a whole file of a worktree's diff, on its new side: the worktree's file on
//! disk. Notes wait, pending, until the person sends them to the worktree's agent, and sending
//! consumes them.
//!
//! The pending notes are kept in `notes.json`, in the worktree's admin directory beside
//! `seen.json`, so they die with the worktree. Each note has an id that is never reused within the
//! worktree, a path relative to the worktree, its anchor, its body, a revision bumped on every
//! edit, and the time it was made. A line or range note also names the copy of the file's content
//! kept when it was made (`notes/<blob id>`), which line translation compares with the file on disk
//! now (`note/translate.rs`). The prompt sending pastes is Orca's (`note/prompt.rs`).
//!
//! Like every state file `notes.json` carries `schema`. Unlike `seen.json`, a file this core cannot
//! use is never written over: it holds the person's words.

pub(crate) mod prompt;
pub(crate) mod translate;

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::fleet::Worktree;
use crate::domain::state::State;

/// The newest schema of `notes.json` this core reads and writes.
pub(crate) const NOTES_SCHEMA: u64 = 1;

/// A note's path: relative to the worktree's root, `/`-separated, with no `..`, so it never leaves
/// the worktree on paper. (The ops check it does not leave it through a symlink either.)
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct NotePath(String);

impl NotePath {
    /// The path the person typed, relative to the worktree's root. `.` parts are dropped; an
    /// absolute path, a `..` part or nothing at all is refused.
    pub(crate) fn parse(text: &str) -> Result<Self, NoteError> {
        let mut parts = Vec::new();
        for component in Path::new(text).components() {
            match component {
                Component::Normal(part) => match part.to_str() {
                    Some(part) => parts.push(part),
                    None => return Err(NoteError::PathNotUtf8),
                },
                Component::CurDir => {}
                Component::ParentDir => return Err(NoteError::PathEscapes(text.to_owned())),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(NoteError::PathAbsolute(text.to_owned()));
                }
            }
        }
        if parts.is_empty() {
            return Err(NoteError::PathEmpty);
        }
        Ok(Self(parts.join("/")))
    }

    /// The path as the prompt and the output show it: `src/app.rs`.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The file this path names in the worktree at `worktree`.
    pub(crate) fn in_worktree(&self, worktree: &Path) -> PathBuf {
        worktree.join(&self.0)
    }
}

impl fmt::Display for NotePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for NotePath {
    type Error = NoteError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let path = Self::parse(&text)?;
        if path.0 != text {
            return Err(NoteError::PathNotPlain(text));
        }
        Ok(path)
    }
}

impl From<NotePath> for String {
    fn from(path: NotePath) -> Self {
        path.0
    }
}

/// An inclusive range of two or more lines, `S-E` with `1 <= S < E`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineRange {
    start: u32,
    end: u32,
}

impl LineRange {
    /// The lines `start` to `end`, both included. Refuses line 0 and a range whose end is not
    /// after its start (one line is a line note, not a range).
    pub(crate) fn new(start: u32, end: u32) -> Result<Self, NoteError> {
        if start == 0 {
            return Err(NoteError::LineZero);
        }
        if end <= start {
            return Err(NoteError::RangeBackwards { start, end });
        }
        Ok(Self { start, end })
    }

    pub(crate) fn start(self) -> u32 {
        self.start
    }

    pub(crate) fn end(self) -> u32 {
        self.end
    }
}

impl FromStr for LineRange {
    type Err = NoteError;

    /// `S-E`, as `--lines` takes it.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bad = || NoteError::RangeSyntax(text.to_owned());
        let (start, end) = text.split_once('-').ok_or_else(bad)?;
        let start = start.trim().parse().map_err(|_| bad())?;
        let end = end.trim().parse().map_err(|_| bad())?;
        Self::new(start, end)
    }
}

/// Where a note is attached in its file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AnchorRecord", into = "AnchorRecord")]
pub(crate) enum Anchor {
    /// One line, numbered from 1.
    Line(u32),
    /// Two or more lines.
    Lines(LineRange),
    /// The whole file.
    File,
}

impl Anchor {
    /// One line, numbered from 1. Refuses line 0.
    pub(crate) fn line(line: u32) -> Result<Self, NoteError> {
        if line == 0 {
            return Err(NoteError::LineZero);
        }
        Ok(Self::Line(line))
    }

    /// Whether the note is on lines, and so needs the file's content kept to be translated.
    pub(crate) fn is_on_lines(self) -> bool {
        !matches!(self, Self::File)
    }

    /// Refuses an anchor on lines past the end of a file of `lines` lines.
    pub(crate) fn check_fits(self, path: &NotePath, lines: usize) -> Result<(), NoteError> {
        let last = match self {
            Self::Line(line) => line,
            Self::Lines(range) => range.end,
            Self::File => return Ok(()),
        };
        if usize::try_from(last).map_or(true, |last| last > lines) {
            return Err(NoteError::OutsideFile {
                path: path.as_str().to_owned(),
                anchor: self,
                lines,
            });
        }
        Ok(())
    }
}

impl fmt::Display for Anchor {
    /// `line 3`, `lines 3-5` or `the whole file`, for a person to read.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line(line) => write!(f, "line {line}"),
            Self::Lines(range) => write!(f, "lines {}-{}", range.start, range.end),
            Self::File => f.write_str("the whole file"),
        }
    }
}

/// An anchor as `notes.json` writes it: `{"scope":"line","line":3}`,
/// `{"scope":"lines","start":3,"end":5}` or `{"scope":"file"}`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
enum AnchorRecord {
    Line { line: u32 },
    Lines { start: u32, end: u32 },
    File,
}

impl TryFrom<AnchorRecord> for Anchor {
    type Error = NoteError;

    fn try_from(record: AnchorRecord) -> Result<Self, Self::Error> {
        match record {
            AnchorRecord::Line { line } => Self::line(line),
            AnchorRecord::Lines { start, end } => LineRange::new(start, end).map(Self::Lines),
            AnchorRecord::File => Ok(Self::File),
        }
    }
}

impl From<Anchor> for AnchorRecord {
    fn from(anchor: Anchor) -> Self {
        match anchor {
            Anchor::Line(line) => Self::Line { line },
            Anchor::Lines(range) => Self::Lines {
                start: range.start,
                end: range.end,
            },
            Anchor::File => Self::File,
        }
    }
}

/// The git blob id of a file's content, as git prints it (40 lowercase hex characters, 64 in a
/// repository using SHA-256): the name of the copy kept for a note on lines.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct BlobId(String);

impl BlobId {
    /// The blob id in `text`, which git printed or a file was named after. Refuses anything else,
    /// so a copy's name never holds a `/`.
    pub(crate) fn parse(text: &str) -> Result<Self, NoteError> {
        let is_hex = text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_hex || !matches!(text.len(), 40 | 64) {
            return Err(NoteError::NotABlobId(text.to_owned()));
        }
        Ok(Self(text.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for BlobId {
    type Error = NoteError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Self::parse(&text)
    }
}

impl From<BlobId> for String {
    fn from(blob: BlobId) -> Self {
        blob.0
    }
}

/// One pending note, as `notes.json` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Note {
    /// Its number in the worktree, from 1, never reused.
    pub(crate) id: u64,
    pub(crate) path: NotePath,
    /// Where it was attached, in the file as it was then.
    pub(crate) anchor: Anchor,
    /// What the person wrote, trimmed, never empty.
    pub(crate) body: String,
    /// 1 when made, bumped by every edit: a note edited while it is being sent is kept.
    pub(crate) rev: u64,
    /// When it was made, in milliseconds since the Unix epoch.
    pub(crate) created: u64,
    /// The copy of the file's content kept when a note on lines was made; `None` for a file note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) copy: Option<BlobId>,
}

/// A note as the commands show it: as recorded, and where its anchor is in the file now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingNote {
    pub(crate) note: Note,
    /// The anchor translated to the file on disk now: the whole file when its lines are gone.
    pub(crate) current: Anchor,
}

/// What `note add` and `note edit` report: the worktree and the note as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Noted {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) note: PendingNote,
}

/// What `note ls` reports: the worktree and its pending notes, in id order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) notes: Vec<PendingNote>,
}

/// What `note rm` reports: the worktree and the note it removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unnoted {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) note: Note,
}

/// What `note send` reports: the worktree, the prompt it pasted, the ids of the notes in it, and
/// those of them kept pending because they were edited while being sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotesSent {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) prompt: String,
    pub(crate) sent: Vec<u64>,
    pub(crate) kept: Vec<u64>,
}

/// A new note, before it has an id: what `note add` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub(crate) path: NotePath,
    pub(crate) anchor: Anchor,
    pub(crate) body: String,
    pub(crate) copy: Option<BlobId>,
    pub(crate) created: u64,
}

/// Every pending note of a worktree, as `notes.json` records them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Notes {
    /// The id the next note gets; ids are never reused, even once their note is gone.
    #[serde(default)]
    next_id: u64,
    /// The pending notes, in id order.
    #[serde(default)]
    notes: Vec<Note>,
}

impl Notes {
    /// The pending notes, in id order.
    pub(crate) fn all(&self) -> &[Note] {
        &self.notes
    }

    /// How many notes are pending.
    pub(crate) fn len(&self) -> usize {
        self.notes.len()
    }

    /// Adds `draft` as a new note, with the next id and revision 1, and returns it.
    pub(crate) fn add(&mut self, draft: Draft) -> Note {
        let highest = self.notes.iter().map(|note| note.id).max().unwrap_or(0);
        let id = self.next_id.max(highest + 1).max(1);
        self.next_id = id + 1;
        let note = Note {
            id,
            path: draft.path,
            anchor: draft.anchor,
            body: draft.body,
            rev: 1,
            created: draft.created,
            copy: draft.copy,
        };
        self.notes.push(note.clone());
        note
    }

    /// Replaces the body of the note `id` with `body`, bumping its revision, and returns it.
    pub(crate) fn edit(&mut self, id: u64, body: String) -> Result<Note, NoteError> {
        let note = self
            .notes
            .iter_mut()
            .find(|note| note.id == id)
            .ok_or(NoteError::NoSuchNote(id))?;
        note.body = body;
        note.rev += 1;
        Ok(note.clone())
    }

    /// Removes the note `id` and returns it.
    pub(crate) fn remove(&mut self, id: u64) -> Result<Note, NoteError> {
        let at = self
            .notes
            .iter()
            .position(|note| note.id == id)
            .ok_or(NoteError::NoSuchNote(id))?;
        Ok(self.notes.remove(at))
    }

    /// The notes `note send` sends: those whose ids are in `ids`, or every one when `ids` is
    /// empty, in id order. Refuses an id with no note, and nothing to send.
    pub(crate) fn select(&self, ids: &[u64]) -> Result<Vec<Note>, NoteError> {
        if let Some(missing) = ids
            .iter()
            .find(|id| !self.notes.iter().any(|n| n.id == **id))
        {
            return Err(NoteError::NoSuchNote(*missing));
        }
        if self.notes.is_empty() {
            return Err(NoteError::NothingPending);
        }
        Ok(self
            .notes
            .iter()
            .filter(|note| ids.is_empty() || ids.contains(&note.id))
            .cloned()
            .collect())
    }

    /// Consumes the notes that were sent, each given as its id and the revision that was sent.
    /// A note edited since (its revision changed) is kept, and so is one already gone. Returns the
    /// ids of the sent notes that were kept because they changed.
    pub(crate) fn consume(&mut self, sent: &[(u64, u64)]) -> Vec<u64> {
        let mut kept = Vec::new();
        self.notes
            .retain(|note| match sent.iter().find(|(id, _)| *id == note.id) {
                Some((_, rev)) if *rev == note.rev => false,
                Some(_) => {
                    kept.push(note.id);
                    true
                }
                None => true,
            });
        kept
    }

    /// The copies some note still refers to; every other copy can go.
    pub(crate) fn copies_in_use(&self) -> BTreeSet<&BlobId> {
        self.notes
            .iter()
            .filter_map(|note| note.copy.as_ref())
            .collect()
    }
}

/// `notes.json` as this core writes it: the notes with their schema.
#[derive(Debug, Serialize)]
struct NotesFile<'a> {
    schema: u64,
    #[serde(flatten)]
    notes: &'a Notes,
}

/// Only the schema of `notes.json`, read before anything else.
#[derive(Debug, Deserialize)]
struct SchemaOnly {
    #[serde(default)]
    schema: Option<u64>,
}

/// What reading `notes.json` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotesRead {
    /// No file: no note was ever made in the worktree, or every one was consumed before the file
    /// existed.
    Missing,
    /// Notes this core can use.
    Usable(Notes),
    /// A file whose schema is newer than this core supports.
    NewerSchema(u64),
    /// A file that is not notes, for the reason given.
    Unreadable(String),
}

impl NotesRead {
    /// How many notes are pending, for `worktree ls`: 0 when there are none this core can use.
    pub(crate) fn count(&self) -> usize {
        match self {
            Self::Usable(notes) => notes.len(),
            Self::Missing | Self::NewerSchema(_) | Self::Unreadable(_) => 0,
        }
    }

    /// The notes, for a command that reads or changes them: none when there is no file. Refuses a
    /// file this core cannot use, rather than reading it as no notes and losing them on the next
    /// write.
    pub(crate) fn usable(self) -> Result<Notes> {
        match self {
            Self::Missing => Ok(Notes::default()),
            Self::Usable(notes) => Ok(notes),
            Self::NewerSchema(schema) => bail!(
                "notes.json has schema {schema}, newer than the schema {NOTES_SCHEMA} this \
                 orca-term supports; upgrade orca-term to use these notes"
            ),
            Self::Unreadable(why) => bail!("notes.json cannot be read ({why}); fix or remove it"),
        }
    }
}

/// What `notes.json`, whose content is `bytes` (`None` when it does not exist), holds.
pub(crate) fn read(bytes: Option<&[u8]>) -> NotesRead {
    let Some(bytes) = bytes else {
        return NotesRead::Missing;
    };
    match serde_json::from_slice::<SchemaOnly>(bytes) {
        Ok(SchemaOnly {
            schema: Some(schema),
        }) if schema > NOTES_SCHEMA => return NotesRead::NewerSchema(schema),
        Ok(_) => {}
        Err(err) => return NotesRead::Unreadable(err.to_string()),
    }
    match serde_json::from_slice::<Notes>(bytes) {
        Ok(notes) => NotesRead::Usable(notes),
        Err(err) => NotesRead::Unreadable(err.to_string()),
    }
}

/// The bytes written to `notes.json`.
pub(crate) fn to_bytes(notes: &Notes) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&NotesFile {
        schema: NOTES_SCHEMA,
        notes,
    })?)
}

/// What the person wrote, trimmed as Orca trims it. Refuses a body with nothing in it.
pub(crate) fn parse_body(text: &str) -> Result<String, NoteError> {
    let body = text.trim();
    if body.is_empty() {
        return Err(NoteError::EmptyBody);
    }
    Ok(body.to_owned())
}

/// How many lines a file whose content is `bytes` has, as an editor numbers them: a last line
/// without a newline still counts, and an empty file has none.
pub(crate) fn line_count(bytes: &[u8]) -> usize {
    // The pieces between newlines: one more than there are newlines.
    let pieces = bytes.split(|&byte| byte == b'\n').count();
    match bytes.last() {
        None | Some(b'\n') => pieces - 1,
        Some(_) => pieces,
    }
}

/// Refuses notes on a worktree whose directory is gone: it has no files to attach them to.
pub(crate) fn check_can_note(worktree: &Worktree) -> Result<()> {
    if let State::Broken(reason) = &worktree.state
        && reason.is_gone()
    {
        bail!(
            "worktree {} at {} is broken ({reason}): its directory is gone, so it takes no notes",
            worktree.name,
            worktree.path.display(),
        );
    }
    Ok(())
}

/// Why a note, or what a note command was given, is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NoteError {
    #[error("a note needs a path, relative to the worktree")]
    PathEmpty,
    #[error("the path {0} is absolute; give it relative to the worktree")]
    PathAbsolute(String),
    #[error("the path {0} leaves the worktree")]
    PathEscapes(String),
    #[error("the path {0} is not written plainly (`a/b`, with no `.` or `//`)")]
    PathNotPlain(String),
    #[error("a note's path must be UTF-8")]
    PathNotUtf8,
    #[error("lines are numbered from 1")]
    LineZero,
    #[error("the range {start}-{end} does not end after it starts; one line is `--line`")]
    RangeBackwards { start: u32, end: u32 },
    #[error("`{0}` is not a range of lines: write it `S-E`, such as `3-5`")]
    RangeSyntax(String),
    #[error("{path} has {lines} lines, so {anchor} is outside it")]
    OutsideFile {
        path: String,
        anchor: Anchor,
        lines: usize,
    },
    #[error("a note needs a body: the text to send")]
    EmptyBody,
    #[error("there is no pending note {0}")]
    NoSuchNote(u64),
    #[error("there are no pending notes to send")]
    NothingPending,
    #[error("`{0}` is not a git blob id")]
    NotABlobId(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::HolderStatus;
    use crate::domain::state::BrokenReason;

    const BLOB: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

    fn path(text: &str) -> NotePath {
        NotePath::parse(text).unwrap()
    }

    fn draft(body: &str) -> Draft {
        Draft {
            path: path("src/app.rs"),
            anchor: Anchor::Line(3),
            body: body.to_owned(),
            copy: Some(BlobId::parse(BLOB).unwrap()),
            created: 1_700_000_000_000,
        }
    }

    #[test]
    fn a_path_is_kept_relative_and_slash_separated() {
        assert_eq!(path("src/app.rs").as_str(), "src/app.rs");
        assert_eq!(path("./src//app.rs").as_str(), "src/app.rs");
        assert_eq!(path("deleted.txt").as_str(), "deleted.txt");
        assert_eq!(
            path("src/app.rs").in_worktree(Path::new("/l/fix")),
            PathBuf::from("/l/fix/src/app.rs")
        );
    }

    #[test]
    fn a_path_that_leaves_the_worktree_or_names_nothing_is_refused() {
        assert_eq!(NotePath::parse(""), Err(NoteError::PathEmpty));
        assert_eq!(NotePath::parse("."), Err(NoteError::PathEmpty));
        assert!(matches!(
            NotePath::parse("/etc/passwd"),
            Err(NoteError::PathAbsolute(_))
        ));
        for text in ["../outside", "a/../../b", "a/.."] {
            assert!(
                matches!(NotePath::parse(text), Err(NoteError::PathEscapes(_))),
                "{text}"
            );
        }
    }

    #[test]
    fn notes_json_takes_only_plain_paths() {
        assert!(NotePath::try_from("src/app.rs".to_owned()).is_ok());
        for text in ["./src/app.rs", "src//app.rs", "../x", "/x", ""] {
            assert!(NotePath::try_from(text.to_owned()).is_err(), "{text}");
        }
    }

    #[test]
    fn a_range_is_two_or_more_lines_from_line_one() {
        let range: LineRange = "3-5".parse().unwrap();
        assert_eq!((range.start(), range.end()), (3, 5));
        assert_eq!("3 - 5".parse::<LineRange>().unwrap(), range);
        assert_eq!(
            "5-5".parse::<LineRange>(),
            Err(NoteError::RangeBackwards { start: 5, end: 5 })
        );
        assert_eq!(
            "5-3".parse::<LineRange>(),
            Err(NoteError::RangeBackwards { start: 5, end: 3 })
        );
        assert_eq!("0-3".parse::<LineRange>(), Err(NoteError::LineZero));
        for text in ["3", "a-b", "3-", "-3", "3-5-7", ""] {
            assert!(
                matches!(text.parse::<LineRange>(), Err(NoteError::RangeSyntax(_))),
                "{text}"
            );
        }
        assert_eq!(Anchor::line(0), Err(NoteError::LineZero));
    }

    #[test]
    fn an_anchor_must_fit_in_its_file() {
        let file = path("a.txt");
        assert!(Anchor::Line(3).check_fits(&file, 3).is_ok());
        assert!(Anchor::Line(4).check_fits(&file, 3).is_err());
        assert!(Anchor::Line(1).check_fits(&file, 0).is_err());
        let range = Anchor::Lines(LineRange::new(2, 3).unwrap());
        assert!(range.check_fits(&file, 3).is_ok());
        let err = Anchor::Lines(LineRange::new(2, 4).unwrap())
            .check_fits(&file, 3)
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "a.txt has 3 lines, so lines 2-4 is outside it"
        );
        // A file note fits any file, even an empty or deleted one.
        assert!(Anchor::File.check_fits(&file, 0).is_ok());
    }

    #[test]
    fn lines_are_counted_as_an_editor_numbers_them() {
        assert_eq!(line_count(b""), 0);
        assert_eq!(line_count(b"a"), 1);
        assert_eq!(line_count(b"a\n"), 1);
        assert_eq!(line_count(b"a\nb"), 2);
        assert_eq!(line_count(b"a\nb\n"), 2);
        assert_eq!(line_count(b"\n\n"), 2);
    }

    #[test]
    fn a_body_is_trimmed_and_never_empty() {
        assert_eq!(parse_body("  why this?\n").unwrap(), "why this?");
        assert_eq!(parse_body("a\nb").unwrap(), "a\nb");
        assert_eq!(parse_body(""), Err(NoteError::EmptyBody));
        assert_eq!(parse_body(" \n\t "), Err(NoteError::EmptyBody));
    }

    #[test]
    fn ids_increase_and_are_never_reused() {
        let mut notes = Notes::default();
        assert_eq!(notes.add(draft("one")).id, 1);
        assert_eq!(notes.add(draft("two")).id, 2);
        notes.remove(2).unwrap();
        assert_eq!(notes.add(draft("three")).id, 3);
        notes.remove(1).unwrap();
        notes.remove(3).unwrap();
        assert_eq!(notes.len(), 0);
        assert_eq!(notes.add(draft("four")).id, 4);
    }

    #[test]
    fn an_edit_replaces_the_body_and_bumps_the_revision() {
        let mut notes = Notes::default();
        let note = notes.add(draft("one"));
        assert_eq!(note.rev, 1);
        let edited = notes.edit(note.id, "uno".to_owned()).unwrap();
        assert_eq!((edited.body.as_str(), edited.rev), ("uno", 2));
        assert_eq!(notes.edit(9, "x".to_owned()), Err(NoteError::NoSuchNote(9)));
        assert_eq!(notes.remove(9), Err(NoteError::NoSuchNote(9)));
    }

    #[test]
    fn sending_selects_the_notes_named_or_all_in_id_order() {
        let mut notes = Notes::default();
        assert_eq!(notes.select(&[]), Err(NoteError::NothingPending));
        for body in ["one", "two", "three"] {
            notes.add(draft(body));
        }
        let ids = |selected: Vec<Note>| selected.iter().map(|n| n.id).collect::<Vec<_>>();
        assert_eq!(ids(notes.select(&[]).unwrap()), vec![1, 2, 3]);
        assert_eq!(ids(notes.select(&[3, 1, 3]).unwrap()), vec![1, 3]);
        assert_eq!(notes.select(&[2, 7]), Err(NoteError::NoSuchNote(7)));
    }

    #[test]
    fn consuming_keeps_a_note_edited_while_it_was_sent() {
        let mut notes = Notes::default();
        for body in ["one", "two", "three"] {
            notes.add(draft(body));
        }
        let sent: Vec<(u64, u64)> = notes
            .select(&[1, 2])
            .unwrap()
            .iter()
            .map(|n| (n.id, n.rev))
            .collect();
        notes.edit(2, "changed".to_owned()).unwrap();
        notes.remove(1).unwrap();
        let kept = notes.consume(&sent);
        assert_eq!(kept, vec![2]);
        let left: Vec<u64> = notes.all().iter().map(|n| n.id).collect();
        assert_eq!(left, vec![2, 3]);
        let sent = vec![(3, 1)];
        assert!(notes.consume(&sent).is_empty());
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn only_copies_a_note_refers_to_are_in_use() {
        let mut notes = Notes::default();
        notes.add(draft("one"));
        notes.add(draft("two"));
        notes.add(Draft {
            anchor: Anchor::File,
            copy: None,
            ..draft("file")
        });
        let in_use: Vec<&str> = notes.copies_in_use().iter().map(|b| b.as_str()).collect();
        assert_eq!(in_use, vec![BLOB]);
    }

    #[test]
    fn notes_json_keeps_its_shape_and_reads_back() {
        let mut notes = Notes::default();
        notes.add(draft("why \"this\"?"));
        notes.add(Draft {
            anchor: Anchor::Lines(LineRange::new(4, 6).unwrap()),
            ..draft("range")
        });
        notes.add(Draft {
            anchor: Anchor::File,
            copy: None,
            ..draft("file")
        });
        let bytes = to_bytes(&notes).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            format!(
                r#"{{"schema":1,"next_id":4,"notes":[{{"id":1,"path":"src/app.rs","anchor":{{"scope":"line","line":3}},"body":"why \"this\"?","rev":1,"created":1700000000000,"copy":"{BLOB}"}},{{"id":2,"path":"src/app.rs","anchor":{{"scope":"lines","start":4,"end":6}},"body":"range","rev":1,"created":1700000000000,"copy":"{BLOB}"}},{{"id":3,"path":"src/app.rs","anchor":{{"scope":"file"}},"body":"file","rev":1,"created":1700000000000}}]}}"#
            )
        );
        assert_eq!(read(Some(&bytes)), NotesRead::Usable(notes));
    }

    #[test]
    fn a_file_this_core_cannot_use_is_never_taken_for_no_notes() {
        assert_eq!(read(None), NotesRead::Missing);
        assert_eq!(read(None).count(), 0);
        assert!(read(None).usable().unwrap().all().is_empty());
        let newer = read(Some(br#"{"schema":2,"notes":[]}"#));
        assert_eq!(newer, NotesRead::NewerSchema(2));
        assert_eq!(newer.count(), 0);
        assert!(newer.usable().unwrap_err().to_string().contains("newer"));
        for bytes in [
            &b"not json"[..],
            br#"{"schema":1,"notes":[{"id":1}]}"#,
            br#"{"schema":1,"notes":[{"id":1,"path":"../x","anchor":{"scope":"file"},"body":"b","rev":1,"created":0}]}"#,
            br#"{"schema":1,"notes":[{"id":1,"path":"x","anchor":{"scope":"lines","start":5,"end":2},"body":"b","rev":1,"created":0}]}"#,
        ] {
            let read = read(Some(bytes));
            assert!(matches!(read, NotesRead::Unreadable(_)), "{read:?}");
            assert!(read.usable().is_err());
        }
    }

    #[test]
    fn a_blob_id_is_a_full_hex_id() {
        assert_eq!(BlobId::parse(BLOB).unwrap().as_str(), BLOB);
        assert!(BlobId::parse(&"b".repeat(64)).is_ok());
        for text in ["", "1a2b3c4", "../../etc/passwd", &BLOB.to_uppercase()] {
            assert!(BlobId::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_worktree_whose_directory_is_gone_takes_no_notes() {
        let worktree = |state| Worktree {
            name: "demo".to_owned(),
            path: PathBuf::from("/l/demo"),
            branch: None,
            state,
            activity: None,
            agent: None,
            holder: HolderStatus::Gone,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
            pr: None,
            last_activity: None,
        };
        let err = check_can_note(&worktree(State::Broken(BrokenReason::DirectoryMissing)));
        assert!(err.unwrap_err().to_string().contains("takes no notes"));
        assert!(check_can_note(&worktree(State::NoAgent)).is_ok());
    }
}
