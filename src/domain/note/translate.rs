//! Domain: line translation, which moves a note's anchor to the line it is on in the worktree's
//! file now. The line a note sends is the line in the current file, the one the agent sees; a
//! note whose lines no longer exist goes as a whole-file note.
//!
//! The note's anchor is a line of the content kept when the note was made. The ops compare that
//! copy with the file on disk now (`git diff --no-index --unified=0`), and this module maps lines
//! over the hunks git printed: a line outside every hunk moves by what the hunks before it added
//! or removed; a line inside a hunk's old side was changed or deleted, so it is gone.
//!
//! Until turns are bounded by snapshots (#38), this is the whole rule: translation is against the
//! content kept when the note was made, not blame over the chain of turn snapshots.

use anyhow::{Context, Result, bail};

use crate::domain::note::{Anchor, LineRange};

/// One hunk of a diff with no context lines (`--unified=0`): the old side's first line and line
/// count, and the new side's line count. With no old lines, the new ones are inserted after old
/// line `old_start` (0 for the top of the file).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hunk {
    old_start: u32,
    old_count: u32,
    new_count: u32,
}

impl Hunk {
    /// Whether the hunk comes wholly before old line `line`, so that it moves the line.
    fn is_before(self, line: u32) -> bool {
        if self.old_count == 0 {
            return self.old_start < line;
        }
        self.old_start + self.old_count - 1 < line
    }

    /// Whether old line `line` is one the hunk changed or deleted.
    fn covers(self, line: u32) -> bool {
        self.old_count > 0 && (self.old_start..self.old_start + self.old_count).contains(&line)
    }

    /// How many lines the hunk adds (positive) or removes (negative).
    fn shift(self) -> i64 {
        i64::from(self.new_count) - i64::from(self.old_count)
    }
}

/// The hunks of the diff git printed, `diff`, in order: every line starting `@@ `. A diff with
/// no hunks (the files are the same) gives none.
pub(crate) fn parse_hunks(diff: &str) -> Result<Vec<Hunk>> {
    diff.lines()
        .filter(|line| line.starts_with("@@ "))
        .map(|line| parse_header(line).with_context(|| format!("git printed the hunk `{line}`")))
        .collect()
}

/// One hunk header, `@@ -a[,b] +c[,d] @@ ...`; a missing count is 1.
fn parse_header(line: &str) -> Result<Hunk> {
    let mut words = line.split(' ').skip(1);
    let (Some(old), Some(new)) = (words.next(), words.next()) else {
        bail!("a hunk header names an old and a new side");
    };
    let (Some(old), Some(new)) = (old.strip_prefix('-'), new.strip_prefix('+')) else {
        bail!("a hunk header's sides start with - and +");
    };
    let (old_start, old_count) = parse_side(old)?;
    let (_, new_count) = parse_side(new)?;
    Ok(Hunk {
        old_start,
        old_count,
        new_count,
    })
}

/// One side of a hunk header, `start[,count]`.
fn parse_side(side: &str) -> Result<(u32, u32)> {
    let (start, count) = side.split_once(',').unwrap_or((side, "1"));
    Ok((start.parse()?, count.parse()?))
}

/// Where old line `line` is in the new file, or `None` when the hunks changed or deleted it.
pub(crate) fn translate_line(line: u32, hunks: &[Hunk]) -> Option<u32> {
    let mut shift: i64 = 0;
    for hunk in hunks {
        if hunk.covers(line) {
            return None;
        }
        if !hunk.is_before(line) {
            break;
        }
        shift += hunk.shift();
    }
    u32::try_from(i64::from(line) + shift).ok()
}

/// Where `anchor` is in the new file: a line that survives moves, a range keeps both ends when
/// both survive, and anything else becomes the whole file.
pub(crate) fn translate(anchor: Anchor, hunks: &[Hunk]) -> Anchor {
    match anchor {
        Anchor::Line(line) => translate_line(line, hunks).map_or(Anchor::File, Anchor::Line),
        Anchor::Lines(range) => {
            let start = translate_line(range.start(), hunks);
            let end = translate_line(range.end(), hunks);
            match (start, end) {
                (Some(start), Some(end)) => {
                    LineRange::new(start, end).map_or(Anchor::File, Anchor::Lines)
                }
                _ => Anchor::File,
            }
        }
        Anchor::File => Anchor::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(old_start: u32, old_count: u32, new_count: u32) -> Hunk {
        Hunk {
            old_start,
            old_count,
            new_count,
        }
    }

    fn lines(start: u32, end: u32) -> Anchor {
        Anchor::Lines(LineRange::new(start, end).unwrap())
    }

    #[test]
    fn hunk_headers_are_read_from_gits_diff() {
        let diff = "diff --git a/x b/x\n\
                    index 1..2 100644\n\
                    --- a/x\n\
                    +++ b/x\n\
                    @@ -0,0 +1,2 @@\n\
                    +new\n\
                    +new\n\
                    @@ -5 +7 @@ fn main() {\n\
                    -old\n\
                    +new\n\
                    @@ -9,3 +10,0 @@\n\
                    -a\n\
                    -b\n\
                    -c\n\
                    \\ No newline at end of file\n";
        assert_eq!(
            parse_hunks(diff).unwrap(),
            vec![hunk(0, 0, 2), hunk(5, 1, 1), hunk(9, 3, 0)]
        );
        assert!(parse_hunks("").unwrap().is_empty());
        assert!(parse_hunks("@@ -x +1 @@\n").is_err());
        assert!(parse_hunks("@@ 5 6 @@\n").is_err());
    }

    #[test]
    fn a_line_moves_down_by_the_lines_inserted_above_it() {
        // Two lines inserted at the top, then one after old line 4.
        let hunks = [hunk(0, 0, 2), hunk(4, 0, 1)];
        assert_eq!(translate_line(1, &hunks), Some(3));
        assert_eq!(translate_line(4, &hunks), Some(6));
        assert_eq!(translate_line(5, &hunks), Some(8));
    }

    #[test]
    fn a_line_moves_up_by_the_lines_deleted_above_it() {
        let hunks = [hunk(2, 3, 0)];
        assert_eq!(translate_line(1, &hunks), Some(1));
        assert_eq!(translate_line(5, &hunks), Some(2));
        assert_eq!(translate_line(10, &hunks), Some(7));
    }

    #[test]
    fn a_line_changed_or_deleted_is_gone() {
        let hunks = [hunk(3, 1, 1), hunk(7, 2, 0)];
        assert_eq!(translate_line(3, &hunks), None);
        assert_eq!(translate_line(7, &hunks), None);
        assert_eq!(translate_line(8, &hunks), None);
        assert_eq!(translate_line(9, &hunks), Some(7));
        assert_eq!(translate(Anchor::Line(3), &hunks), Anchor::File);
    }

    #[test]
    fn a_hunk_that_replaces_with_more_lines_moves_what_follows() {
        let hunks = [hunk(2, 1, 4)];
        assert_eq!(translate_line(1, &hunks), Some(1));
        assert_eq!(translate_line(2, &hunks), None);
        assert_eq!(translate_line(3, &hunks), Some(6));
    }

    #[test]
    fn adjacent_hunks_add_up() {
        let hunks = [hunk(2, 1, 0), hunk(3, 1, 2), hunk(4, 0, 1)];
        assert_eq!(translate_line(1, &hunks), Some(1));
        assert_eq!(translate_line(2, &hunks), None);
        assert_eq!(translate_line(3, &hunks), None);
        // Old line 4: -1, then +1, and the insertion after it does not count.
        assert_eq!(translate_line(4, &hunks), Some(4));
        assert_eq!(translate_line(5, &hunks), Some(6));
    }

    #[test]
    fn the_end_of_the_file_moves_like_any_line() {
        // A 5-line file whose last line is deleted, then 3 lines appended.
        assert_eq!(translate_line(5, &[hunk(5, 1, 0)]), None);
        assert_eq!(translate_line(5, &[hunk(5, 0, 3)]), Some(5));
        assert_eq!(translate_line(4, &[hunk(5, 1, 0)]), Some(4));
    }

    #[test]
    fn a_file_emptied_loses_every_line() {
        let hunks = [hunk(1, 3, 0)];
        for line in 1..=3 {
            assert_eq!(translate_line(line, &hunks), None);
        }
        assert_eq!(translate(lines(1, 3), &hunks), Anchor::File);
    }

    #[test]
    fn no_hunks_leave_every_line_where_it_was() {
        assert_eq!(translate(Anchor::Line(7), &[]), Anchor::Line(7));
        assert_eq!(translate(lines(2, 4), &[]), lines(2, 4));
        assert_eq!(translate(Anchor::File, &[hunk(1, 1, 0)]), Anchor::File);
    }

    #[test]
    fn a_range_keeps_both_ends_or_becomes_the_whole_file() {
        let inserted = [hunk(0, 0, 2)];
        assert_eq!(translate(lines(2, 4), &inserted), lines(4, 6));
        // Inside the range: both ends survive, and the range grows.
        assert_eq!(translate(lines(2, 4), &[hunk(2, 0, 3)]), lines(2, 7));
        assert_eq!(translate(lines(2, 4), &[hunk(2, 1, 1)]), Anchor::File);
        assert_eq!(translate(lines(2, 4), &[hunk(4, 1, 0)]), Anchor::File);
        // The lines between the ends deleted: both ends survive, side by side.
        assert_eq!(translate(lines(2, 4), &[hunk(3, 1, 0)]), lines(2, 3));
    }
}
