//! App: the right column's card (#177, the design's `right2`): `changes` with how many files and
//! the lines added and deleted, each changed file with its letter, name, directory and counts, a
//! diffstat bar; then `pull request`, its number and title, its state and how many checks fail and
//! run, and each check; then `branch`, how far it is ahead of and behind the local default branch.
//! With the focus, the file under the cursor takes the accent; enter opens its diff in Neovim.

use ratatui::layout::Rect;

use crate::app::menu;
use crate::app::paint::{Painter, Seg, bold, fg, seg, segs_width, trunc, trunc_left, width};
use crate::app::state::{App, Focus};
use crate::app::theme::{
    ACCENT, BORDER, CANVAS, DONE, FAILED, FOCUS_SEL, PANEL, TEXT, TEXT_2, WORKING,
};
use crate::domain::changes::{Change, ChangedFile, Changes, Lines};
use crate::domain::fleet::Worktree;
use crate::domain::pr::{CheckResult, Found, PullRequest};

/// The card's title.
const TITLE: &str = "changes";

/// The rows after the list of files that do not depend on the pull request: a blank row, the
/// diffstat bar, two blank rows, `pull request` and a blank row; after the pull request two blank
/// rows, `branch`, a blank row and its two lines.
const FIXED_ROWS: u16 = 12;

pub(crate) fn draw(p: &mut Painter<'_>, app: &App, card: Rect) {
    let focused = app.focus == Focus::RightColumn;
    p.card(card, PANEL, CANVAS, if focused { width(TITLE) } else { 0 });
    let column = Column {
        x: card.x + 2,
        end: card.right() - 2,
        bottom: card.bottom() - 1,
    };
    let worktree = app
        .open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open))
        .map(|(_, worktree)| worktree);
    let title = if focused { ACCENT } else { TEXT };
    let changes = app.open_changes();
    let count = changes.map_or(String::new(), |changes| {
        format!("  {}", changes.files.len())
    });
    p.line(
        column.x,
        card.y + 1,
        &[seg(TITLE, bold(title)), seg(count, fg(TEXT_2))],
    );
    let Some(changes) = changes else {
        let why = app.changes_error().unwrap_or("reading…");
        column.put(p, card.y + 3, &[seg(why, fg(TEXT_2))]);
        return;
    };
    p.rsegs(column.end, card.y + 1, &totals(changes.totals()));
    let pr = worktree.and_then(pull_request);
    let tail = FIXED_ROWS + pr.map_or(1, |pr| 3 + pr_check_rows(pr));
    let room = (column.bottom.saturating_sub(card.y + 3 + tail)).max(1);
    let mut row = files(p, app, &column, card, changes, room, focused);
    row += 1;
    bar(p, &column, row, changes.totals());
    row += 3;
    row = pull_request_section(p, &column, row, worktree);
    row += 2;
    branch_section(p, &column, row, worktree, changes);
}

/// Where the card's content goes: from column `x` to just before `end`, above row `bottom`.
struct Column {
    x: u16,
    end: u16,
    bottom: u16,
}

impl Column {
    /// Pieces at `row`, cut to the column, when the row is inside the card.
    fn put(&self, p: &mut Painter<'_>, row: u16, segs: &[Seg]) {
        if row < self.bottom {
            p.segs(self.x, row, segs, self.end - self.x);
        }
    }

    /// A section's label at `row`, bold, then a rule to the column's end: `pull request ───`.
    fn label(&self, p: &mut Painter<'_>, row: u16, text: &str) {
        if row >= self.bottom {
            return;
        }
        let after = p.text(self.x, row, text, bold(TEXT_2)) + 1;
        let rule = "─".repeat(usize::from(self.end.saturating_sub(after)));
        p.text(after, row, &rule, fg(BORDER));
    }
}

/// `+55 −21`.
fn totals(lines: Lines) -> Vec<Seg> {
    vec![
        seg(format!("+{}", lines.added), fg(DONE)),
        seg(format!(" −{}", lines.deleted), fg(FAILED)),
    ]
}

/// The changed files in at most `room` rows from the card's fourth, scrolled so the cursor shows.
/// Returns the row after the last.
fn files(
    p: &mut Painter<'_>,
    app: &App,
    column: &Column,
    card: Rect,
    changes: &Changes,
    room: u16,
    focused: bool,
) -> u16 {
    let mut row = card.y + 3;
    if changes.files.is_empty() {
        column.put(p, row, &[seg("no changes", fg(TEXT_2))]);
        return row + 1;
    }
    let cursor = app.file_cursor.min(changes.files.len() - 1);
    let offset = (cursor + 1).saturating_sub(usize::from(room));
    for (index, file) in changes
        .files
        .iter()
        .enumerate()
        .skip(offset)
        .take(usize::from(room))
    {
        let under_cursor = focused && index == cursor;
        if under_cursor && row < column.bottom {
            p.tint(Rect::new(card.x + 1, row, card.width - 2, 1), FOCUS_SEL);
            p.text(card.x + 1, row, "▎", fg(ACCENT).bg(FOCUS_SEL.color()));
        }
        file_row(p, column, row, file, under_cursor);
        row += 1;
    }
    row
}

/// One file: `M login.rs src/auth/  +18 −9`, the directory cut from the left to fit.
fn file_row(p: &mut Painter<'_>, column: &Column, row: u16, file: &ChangedFile, cursor: bool) {
    if row >= column.bottom {
        return;
    }
    let letter_tok = match file.change {
        Change::Added => DONE,
        Change::Deleted => FAILED,
        Change::Modified | Change::Renamed => WORKING,
    };
    p.text(
        column.x,
        row,
        &file.change.letter().to_string(),
        bold(letter_tok),
    );
    let counts = file.lines.map(counts).unwrap_or_default();
    let counts_x = column.end - segs_width(&counts);
    p.line(counts_x, row, &counts);
    let (dir, base) = match file.path.rfind('/') {
        Some(at) => (&file.path[..=at], &file.path[at + 1..]),
        None => ("", file.path.as_str()),
    };
    let name_style = match (cursor, file.change) {
        (true, _) => bold(ACCENT),
        (false, Change::Deleted) => fg(TEXT_2),
        (false, _) => fg(TEXT),
    };
    let name_x = column.x + 2;
    let room = counts_x.saturating_sub(name_x + 1);
    let after = p.text(name_x, row, &trunc(base, room), name_style);
    let dir_room = counts_x.saturating_sub(after + 2);
    if !dir.is_empty() && dir_room > 1 {
        p.text(after + 1, row, &trunc_left(dir, dir_room), fg(TEXT_2));
    }
}

/// `+18 −9`, leaving out a side with nothing.
fn counts(lines: Lines) -> Vec<Seg> {
    let mut out = Vec::new();
    if lines.added > 0 {
        out.push(seg(format!("+{}", lines.added), fg(DONE)));
    }
    if lines.deleted > 0 {
        let lead = if lines.added > 0 { " " } else { "" };
        out.push(seg(format!("{lead}−{}", lines.deleted), fg(FAILED)));
    }
    out
}

/// The diffstat bar, `━` across the column: the added lines' share in DONE, the rest in FAILED.
fn bar(p: &mut Painter<'_>, column: &Column, row: u16, lines: Lines) {
    let total = lines.added + lines.deleted;
    if total == 0 || row >= column.bottom {
        return;
    }
    let length = u64::from(column.end - column.x);
    let added = u16::try_from((length * lines.added + total / 2) / total).unwrap_or(0);
    let deleted = column.end - column.x - added;
    let at = p.text(column.x, row, &"━".repeat(usize::from(added)), fg(DONE));
    p.text(at, row, &"━".repeat(usize::from(deleted)), fg(FAILED));
}

/// The worktree's pull request, when the last `pr refresh` found one.
fn pull_request(worktree: &Worktree) -> Option<&PullRequest> {
    match &worktree.pr.as_ref()?.found {
        Found::Pr(pr) => Some(pr),
        Found::NoPr | Found::Unavailable(_) => None,
    }
}

/// The rows a pull request's checks take: one per check, or one for the checks in a word.
fn pr_check_rows(pr: &PullRequest) -> u16 {
    u16::try_from(pr.runs.len()).unwrap_or(u16::MAX).max(1)
}

/// `pull request`, then the pull request: its number and title, `draft · 1 failing · 1 running`
/// and its checks; or `○ none yet`, or why gh could not say. Returns the row after it.
fn pull_request_section(
    p: &mut Painter<'_>,
    column: &Column,
    row: u16,
    worktree: Option<&Worktree>,
) -> u16 {
    column.label(p, row, "pull request");
    let row = row + 2;
    let found = worktree
        .and_then(|worktree| worktree.pr.as_ref())
        .map(|facts| &facts.found);
    match found {
        Some(Found::Pr(pr)) => {
            column.put(
                p,
                row,
                &[
                    seg(format!("#{}", pr.number), bold(TEXT)),
                    seg(format!(" {}", pr.title), fg(TEXT)),
                ],
            );
            column.put(p, row + 1, &[seg(pr_summary(pr), fg(TEXT_2))]);
            checks(p, column, row + 3, pr)
        }
        Some(Found::Unavailable(why)) => {
            column.put(
                p,
                row,
                &[seg("?  ", fg(TEXT_2)), seg(why.as_str(), fg(TEXT_2))],
            );
            row + 1
        }
        Some(Found::NoPr) | None => {
            column.put(
                p,
                row,
                &[seg("○", fg(TEXT_2)), seg("  none yet", fg(TEXT_2))],
            );
            row + 1
        }
    }
}

/// `draft · 1 failing · 1 running`: the state, then the checks failing and running, when any.
fn pr_summary(pr: &PullRequest) -> String {
    let mut parts = vec![pr.state.name().to_owned()];
    let count = |result: CheckResult| pr.runs.iter().filter(|run| run.result == result).count();
    let (failing, running) = (count(CheckResult::Fail), count(CheckResult::Pending));
    if failing > 0 {
        parts.push(format!("{failing} failing"));
    }
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    parts.join(" · ")
}

/// Each check, its glyph then its name, a failed one's name in FAILED; for a pull request
/// recorded before the checks were kept one by one, the checks in a word. Returns the row after.
fn checks(p: &mut Painter<'_>, column: &Column, mut row: u16, pr: &PullRequest) -> u16 {
    if pr.runs.is_empty() {
        column.put(
            p,
            row,
            &[seg(format!("checks {}", pr.checks.name()), fg(TEXT_2))],
        );
        return row + 1;
    }
    for run in &pr.runs {
        let (glyph, tok) = match run.result {
            CheckResult::Pass => ('✓', DONE),
            CheckResult::Fail => ('✗', FAILED),
            CheckResult::Pending => ('◐', WORKING),
            CheckResult::Skip => ('-', TEXT_2),
        };
        let name = if run.result == CheckResult::Fail {
            FAILED
        } else {
            TEXT
        };
        column.put(
            p,
            row,
            &[
                seg(glyph.to_string(), bold(tok)),
                seg(" ", fg(TEXT)),
                seg(run.name.as_str(), fg(name)),
            ],
        );
        row += 1;
    }
    row
}

/// `branch`, then `fix-login → main` and `↑2 ahead  ↓0 behind`.
fn branch_section(
    p: &mut Painter<'_>,
    column: &Column,
    row: u16,
    worktree: Option<&Worktree>,
    changes: &Changes,
) {
    column.label(p, row, "branch");
    let row = row + 2;
    let branch = worktree
        .and_then(|worktree| worktree.branch.as_ref())
        .map_or("no branch", |branch| branch.as_str());
    let Some(drift) = &changes.drift else {
        column.put(p, row, &[seg(branch, fg(TEXT))]);
        column.put(p, row + 1, &[seg("no local default branch", fg(TEXT_2))]);
        return;
    };
    column.put(
        p,
        row,
        &[
            seg(branch, fg(TEXT)),
            seg(" → ", fg(TEXT_2)),
            seg(drift.against.as_str(), fg(TEXT)),
        ],
    );
    column.put(
        p,
        row + 1,
        &[
            seg(format!("↑{}", drift.ahead), bold(TEXT)),
            seg(" ahead  ", fg(TEXT_2)),
            seg(format!("↓{}", drift.behind), bold(TEXT)),
            seg(" behind", fg(TEXT_2)),
        ],
    );
}
