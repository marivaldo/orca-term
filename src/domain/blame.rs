//! Domain: which turn wrote each line of a worktree's file as it is on disk now (`turn blame`,
//! #38). git blames the file on disk against the tip of the worktree's chain of snapshots
//! (`git blame --porcelain --contents`, `domain/snapshot.rs`), and each line's commit is mapped
//! through the turn log:
//!
//! - a commit that is some turn's end snapshot: that turn wrote the line;
//! - any other snapshot of the chain (a begin alone, holding the person's edits between turns, or
//!   the chain's first snapshot, holding the worktree's files as they were before any turn):
//!   no turn, the line was written between turns;
//! - git's all-zero commit, "not committed yet": the line changed since the latest snapshot.
//!   During an open turn those changes belong to it once it ends.

use std::path::{Component, Path};

use anyhow::{Result, bail};

use crate::domain::history::LoggedTurn;

/// The commit git's blame gives a line it found in no commit: changed in the file on disk.
const NOT_COMMITTED: char = '0';

/// One line of the file, as `git blame --porcelain` gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlamedLine {
    /// The commit git found the line in, the all-zero id when it is in none.
    pub(crate) commit: String,
    /// The line's number in the file on disk, from 1.
    pub(crate) line: usize,
    /// The line's text, without its newline.
    pub(crate) text: String,
}

/// Who wrote a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Source {
    /// The turn `number`, of the agent `agent` (unknown for a turn logged before it was recorded).
    Turn { number: u64, agent: Option<String> },
    /// No turn: the person, between turns, or the worktree's files before the first turn.
    BetweenTurns,
    /// Changed since the latest snapshot.
    SinceSnapshot,
}

/// A line of the file and who wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LineAuthor {
    /// The line's number in the file on disk, from 1.
    pub(crate) line: usize,
    pub(crate) source: Source,
    /// The snapshot git found the line in; `None` for a line changed since the latest one.
    pub(crate) snapshot: Option<String>,
    pub(crate) text: String,
}

/// What `turn blame` found for a file of a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Blame {
    /// The worktree's name.
    pub(crate) name: String,
    /// The worktree's path.
    pub(crate) path: std::path::PathBuf,
    /// The file, relative to the worktree's root, `/`-separated.
    pub(crate) file: String,
    /// Each line of the file on disk, in order.
    pub(crate) lines: Vec<LineAuthor>,
}

/// The lines `git blame --porcelain` printed as `text`, in the file's order. Each line of the file
/// is a header (`<commit> <original line> <final line> [<lines in group>]`), lines about the
/// commit the first time it appears (`author ...`, `summary ...`), then the line's text after a
/// tab.
pub(crate) fn parse_porcelain(text: &str) -> Result<Vec<BlamedLine>> {
    let mut lines = Vec::new();
    let mut header: Option<(String, usize)> = None;
    for row in text.split('\n') {
        if let Some(content) = row.strip_prefix('\t') {
            let Some((commit, line)) = header.take() else {
                bail!("git blame printed a line's text before its header");
            };
            lines.push(BlamedLine {
                commit,
                line,
                text: content.to_owned(),
            });
        } else if let Some(parsed) = parse_header(row) {
            header = Some(parsed);
        }
    }
    lines.sort_by_key(|blamed| blamed.line);
    Ok(lines)
}

/// The commit and the final line number of a porcelain header row, `None` for any other row. A
/// row about a commit starts with a word (`author`, `previous`), never with a full id.
fn parse_header(row: &str) -> Option<(String, usize)> {
    let mut words = row.split(' ');
    let commit = words.next()?;
    let is_id = matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !is_id {
        return None;
    }
    let _original: usize = words.next()?.parse().ok()?;
    let line: usize = words.next()?.parse().ok()?;
    Some((commit.to_owned(), line))
}

/// Who wrote each of `blamed`, by the turns `turns` of the worktree: see the module's comment.
pub(crate) fn attribute(blamed: Vec<BlamedLine>, turns: &[LoggedTurn]) -> Vec<LineAuthor> {
    blamed
        .into_iter()
        .map(|blamed| {
            let (source, snapshot) = if is_not_committed(&blamed.commit) {
                (Source::SinceSnapshot, None)
            } else {
                (source_of(&blamed.commit, turns), Some(blamed.commit))
            };
            LineAuthor {
                line: blamed.line,
                source,
                snapshot,
                text: blamed.text,
            }
        })
        .collect()
}

/// The turn whose end snapshot is `commit`, else no turn.
fn source_of(commit: &str, turns: &[LoggedTurn]) -> Source {
    turns
        .iter()
        .find(|turn| turn.end.as_deref() == Some(commit))
        .map_or(Source::BetweenTurns, |turn| Source::Turn {
            number: turn.number,
            agent: turn.agent.clone(),
        })
}

/// Whether `commit` is git's all-zero id for a line in no commit.
fn is_not_committed(commit: &str) -> bool {
    !commit.is_empty() && commit.chars().all(|c| c == NOT_COMMITTED)
}

/// Every line of `content`, a file no snapshot holds yet (or with no snapshot at all), changed
/// since the latest snapshot.
pub(crate) fn unsnapshotted(content: &str) -> Vec<LineAuthor> {
    content
        .lines()
        .enumerate()
        .map(|(index, text)| LineAuthor {
            line: index + 1,
            source: Source::SinceSnapshot,
            snapshot: None,
            text: text.to_owned(),
        })
        .collect()
}

/// The file `given` names in the worktree at `worktree`, relative to its root and `/`-separated:
/// `given` is relative to the root, or absolute inside it (both already resolved through
/// symlinks by the caller). `.` parts are dropped. Refuses a path outside the worktree, one with
/// a `..` part, one that is not UTF-8, and nothing at all.
pub(crate) fn worktree_file(given: &Path, worktree: &Path) -> Result<String> {
    let relative = if given.is_absolute() {
        let Ok(inside) = given.strip_prefix(worktree) else {
            bail!(
                "{} is outside the worktree {}",
                given.display(),
                worktree.display()
            );
        };
        inside
    } else {
        given
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => match part.to_str() {
                Some(part) => parts.push(part),
                None => bail!("the path {} is not UTF-8", given.display()),
            },
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("the path {} leaves the worktree", given.display());
            }
        }
    }
    if parts.is_empty() {
        bail!("name a file of the worktree, relative to its root");
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::history::Progress;
    use crate::domain::turn::Outcome;

    const END1: &str = "1111111111111111111111111111111111111111";
    const PERSON: &str = "2222222222222222222222222222222222222222";
    const ZERO: &str = "0000000000000000000000000000000000000000";

    fn ended(number: u64, end: &str) -> LoggedTurn {
        LoggedTurn {
            number,
            agent: Some("claude".to_owned()),
            session: None,
            prompt: String::new(),
            started_at: None,
            ended_at: None,
            progress: Progress::Ended(Outcome::Finished),
            end_observed: Some(true),
            begin: None,
            end: Some(end.to_owned()),
            denials: 0,
        }
    }

    fn porcelain() -> String {
        [
            format!("{PERSON} 1 1 1"),
            "author person".to_owned(),
            "summary orca-term: begin of turn 1".to_owned(),
            "filename a.rs".to_owned(),
            "\t# demo".to_owned(),
            format!("{END1} 1 2 1"),
            "author claude".to_owned(),
            format!("previous {PERSON} a.rs"),
            "filename a.rs".to_owned(),
            "\tfn main() {".to_owned(),
            format!("{ZERO} 3 3 1"),
            "author Not Committed Yet".to_owned(),
            "filename a.rs".to_owned(),
            "\t}".to_owned(),
            String::new(),
        ]
        .join("\n")
    }

    #[test]
    fn porcelain_gives_each_line_its_commit_and_text() {
        let lines = parse_porcelain(&porcelain()).unwrap();
        assert_eq!(
            lines,
            [
                BlamedLine {
                    commit: PERSON.to_owned(),
                    line: 1,
                    text: "# demo".to_owned()
                },
                BlamedLine {
                    commit: END1.to_owned(),
                    line: 2,
                    text: "fn main() {".to_owned()
                },
                BlamedLine {
                    commit: ZERO.to_owned(),
                    line: 3,
                    text: "}".to_owned()
                },
            ]
        );
        assert!(parse_porcelain("\torphan text").is_err());
        assert_eq!(parse_porcelain("").unwrap(), []);
    }

    #[test]
    fn an_end_snapshot_is_its_turn_any_other_snapshot_no_turn_and_zero_since_the_last() {
        let lines = parse_porcelain(&porcelain()).unwrap();
        let authors = attribute(lines, &[ended(1, END1)]);
        assert_eq!(
            authors
                .iter()
                .map(|author| (author.line, author.source.clone(), author.snapshot.clone()))
                .collect::<Vec<_>>(),
            [
                (1, Source::BetweenTurns, Some(PERSON.to_owned())),
                (
                    2,
                    Source::Turn {
                        number: 1,
                        agent: Some("claude".to_owned())
                    },
                    Some(END1.to_owned())
                ),
                (3, Source::SinceSnapshot, None),
            ]
        );
    }

    #[test]
    fn a_file_no_snapshot_holds_is_all_since_the_last_snapshot() {
        let lines = unsnapshotted("a\nb\n");
        assert_eq!(lines.len(), 2);
        assert!(
            lines
                .iter()
                .all(|line| line.source == Source::SinceSnapshot && line.snapshot.is_none())
        );
        assert_eq!((lines[1].line, lines[1].text.as_str()), (2, "b"));
    }

    #[test]
    fn a_path_is_relative_to_the_worktree_or_absolute_inside_it() {
        let root = Path::new("/r/wt");
        assert_eq!(
            worktree_file(Path::new("src/a.rs"), root).unwrap(),
            "src/a.rs"
        );
        assert_eq!(
            worktree_file(Path::new("./src/a.rs"), root).unwrap(),
            "src/a.rs"
        );
        assert_eq!(
            worktree_file(Path::new("/r/wt/src/a.rs"), root).unwrap(),
            "src/a.rs"
        );
        for refused in [
            "/r/other/a.rs",
            "../a.rs",
            "src/../../a.rs",
            "",
            ".",
            "/r/wt",
        ] {
            assert!(
                worktree_file(Path::new(refused), root).is_err(),
                "{refused}"
            );
        }
    }
}
