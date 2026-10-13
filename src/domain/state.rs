//! Domain: a worktree's state, kept as `worktree.json` in git's per-worktree admin directory
//! (ADR 0003).
//!
//! Every state file carries `schema`. Reading tolerates any schema, taking the fields it knows and
//! ignoring the rest; writing refuses a file whose schema is newer than this core supports, so an
//! older core never clobbers what a newer one wrote.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The newest schema this core reads and writes.
pub(crate) const SUPPORTED_SCHEMA: u64 = 1;

/// The worktree's state file, inside its admin directory.
const WORKTREE_FILE: &str = "worktree.json";

/// The directory, inside git's per-worktree admin directory, that holds the worktree's state.
pub(crate) const ADMIN_DIR_NAME: &str = "orca-term";

/// The directory, inside git's per-worktree admin directory, that holds one worktree's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminDir(PathBuf);

impl AdminDir {
    /// The admin directory git reported at `path`.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Where the directory is.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The worktree's state file, `worktree.json`, inside this directory.
    pub(crate) fn state_file(&self) -> PathBuf {
        self.0.join(WORKTREE_FILE)
    }
}

/// What a worktree is doing, as the fleet shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// No agent is attached to the worktree.
    NoAgent,
    /// The worktree cannot be used, for the reason given.
    Broken(BrokenReason),
}

impl State {
    /// How a table shows the state.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::NoAgent => "no agent",
            Self::Broken(_) => "broken",
        }
    }
}

/// Why a worktree is broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrokenReason {
    /// Its directory is missing.
    DirectoryMissing,
    /// Git marks it prunable, for the reason git gives (its `.git` file is gone, say).
    Prunable(String),
    /// Its state file cannot be read, for the reason given.
    StateUnreadable(String),
}

impl BrokenReason {
    /// Whether the worktree is gone from disk, so that only `worktree prune` can clean it.
    pub(crate) fn is_gone(&self) -> bool {
        match self {
            Self::DirectoryMissing | Self::Prunable(_) => true,
            Self::StateUnreadable(_) => false,
        }
    }
}

/// The reason as the fleet shows it, in the table's detail and in the JSON `detail`.
impl fmt::Display for BrokenReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryMissing => {
                write!(f, "directory missing; run orca-term worktree prune")
            }
            Self::Prunable(reason) => write!(
                f,
                "git marks it prunable ({reason}); run orca-term worktree prune"
            ),
            Self::StateUnreadable(reason) => write!(f, "{reason}"),
        }
    }
}

/// `worktree.json` as this core writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorktreeFile {
    pub(crate) schema: u64,
    pub(crate) agent: Option<String>,
}

impl WorktreeFile {
    /// The state of a worktree just created: no agent.
    pub(crate) fn fresh() -> Self {
        Self {
            schema: SUPPORTED_SCHEMA,
            agent: None,
        }
    }

    /// The bytes written to `worktree.json`.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
}

/// `worktree.json` as this core reads it, from any schema.
#[derive(Debug, Default, Deserialize)]
struct Seen {
    #[serde(default)]
    schema: Option<u64>,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "only `no_agent` exists so far; a recorded agent gains its states in a later slice"
    )]
    agent: serde_json::Value,
}

/// The state recorded in the state file at `path`, whose content is `bytes`, or `None` when the
/// file does not exist: a worktree with no agent.
pub(crate) fn read(path: &Path, bytes: Option<&[u8]>) -> Result<State> {
    // Parsed even though only `no_agent` exists yet, so a corrupt file is reported, not hidden.
    if let Some(bytes) = bytes {
        parse(path, bytes)?;
    }
    Ok(State::NoAgent)
}

/// Refuses to overwrite the state file at `path`, whose content is `existing` (`None` when it does
/// not exist), when it is corrupt or carries a schema newer than this core supports.
pub(crate) fn check_overwrite(path: &Path, existing: Option<&[u8]>) -> Result<()> {
    let Some(bytes) = existing else {
        return Ok(());
    };
    if let Some(schema) = parse(path, bytes)?.schema
        && schema > SUPPORTED_SCHEMA
    {
        bail!(
            "{} has schema {schema}, newer than the schema {SUPPORTED_SCHEMA} this orca-term \
             supports; upgrade orca-term to change this worktree",
            path.display()
        );
    }
    Ok(())
}

fn parse(path: &Path, bytes: &[u8]) -> Result<Seen> {
    serde_json::from_slice(bytes).with_context(|| format!("could not parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/admin/orca-term/worktree.json";

    #[test]
    fn writes_schema_1_with_no_agent() {
        let bytes = WorktreeFile::fresh().to_bytes().unwrap();
        assert_eq!(bytes, br#"{"schema":1,"agent":null}"#);
        assert_eq!(read(Path::new(PATH), Some(&bytes)).unwrap(), State::NoAgent);
    }

    #[test]
    fn refuses_to_write_over_a_newer_schema() {
        let newer = br#"{"schema":2,"agent":null,"holder":{"pid":1}}"#;
        let err = check_overwrite(Path::new(PATH), Some(newer)).unwrap_err();
        assert!(err.to_string().contains("upgrade orca-term"), "{err}");
    }

    #[test]
    fn overwrites_a_file_of_the_supported_schema_or_no_file() {
        check_overwrite(Path::new(PATH), Some(br#"{"schema":1,"agent":null}"#)).unwrap();
        check_overwrite(Path::new(PATH), None).unwrap();
    }

    #[test]
    fn reads_any_schema_and_ignores_unknown_fields() {
        let bytes = br#"{"schema":7,"agent":null,"future":[1,2]}"#;
        assert_eq!(read(Path::new(PATH), Some(bytes)).unwrap(), State::NoAgent);
    }

    #[test]
    fn a_missing_file_is_a_worktree_with_no_agent() {
        assert_eq!(read(Path::new(PATH), None).unwrap(), State::NoAgent);
    }

    #[test]
    fn the_state_file_lives_in_the_admin_dir() {
        let admin = AdminDir::new(PathBuf::from("/repo/.git/worktrees/fix/orca-term"));
        assert_eq!(
            admin.state_file(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/worktree.json")
        );
    }

    #[test]
    fn a_broken_reason_says_what_to_do() {
        assert_eq!(
            BrokenReason::DirectoryMissing.to_string(),
            "directory missing; run orca-term worktree prune"
        );
        assert_eq!(
            BrokenReason::Prunable("gitdir file points to non-existent location".to_owned())
                .to_string(),
            "git marks it prunable (gitdir file points to non-existent location); run orca-term \
             worktree prune"
        );
        assert_eq!(
            BrokenReason::StateUnreadable("could not parse x".to_owned()).to_string(),
            "could not parse x"
        );
    }

    #[test]
    fn only_a_worktree_gone_from_disk_is_left_to_prune() {
        assert!(BrokenReason::DirectoryMissing.is_gone());
        assert!(BrokenReason::Prunable("gone".to_owned()).is_gone());
        assert!(!BrokenReason::StateUnreadable("corrupt".to_owned()).is_gone());
    }

    #[test]
    fn a_corrupt_file_is_reported_with_its_path() {
        let err = read(Path::new(PATH), Some(b"{not json")).unwrap_err();
        assert!(format!("{err:#}").starts_with("could not parse /admin/orca-term/worktree.json: "));
    }
}
