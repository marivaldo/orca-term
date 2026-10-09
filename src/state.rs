//! A lane's state: plain files in git's per-worktree admin directory (ADR 0003).
//!
//! Every state file carries `schema`. Reading tolerates any schema, taking the fields it knows and
//! ignoring the rest; writing refuses a file whose schema is newer than this core supports, so an
//! older core never clobbers what a newer one wrote.

use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::git;

/// The newest schema this core reads and writes.
pub const SUPPORTED_SCHEMA: u64 = 1;

/// The lane's state file, inside its admin directory.
const LANE_FILE: &str = "lane.json";

/// What a lane is doing, as the fleet shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// No agent is attached to the lane.
    NoAgent,
    /// The lane's directory is gone, or its state cannot be read.
    Broken,
}

impl State {
    /// How a table shows the state.
    pub fn label(self) -> &'static str {
        match self {
            Self::NoAgent => "no agent",
            Self::Broken => "broken",
        }
    }
}

/// `lane.json` as this core writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaneFile {
    pub schema: u64,
    pub agent: Option<String>,
}

impl LaneFile {
    /// The state of a lane just created: no agent.
    pub fn fresh() -> Self {
        Self {
            schema: SUPPORTED_SCHEMA,
            agent: None,
        }
    }
}

/// `lane.json` as this core reads it, from any schema.
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

/// The admin directory holding the state of the lane whose worktree is `lane`.
pub fn admin_dir(lane: &Path) -> Result<PathBuf> {
    let dir = git::text(
        lane,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "orca-term",
        ],
    )?;
    Ok(PathBuf::from(dir))
}

/// The state recorded in `admin_dir`. A missing `lane.json` is a lane with no agent. Never writes.
pub fn read(admin_dir: &Path) -> Result<State> {
    // Parsed even though only `no_agent` exists yet, so a corrupt file is reported, not hidden.
    read_seen(&admin_dir.join(LANE_FILE))?;
    Ok(State::NoAgent)
}

/// Writes `file` as `lane.json` in `admin_dir`, atomically, refusing to overwrite a newer schema.
pub fn write(admin_dir: &Path, file: &LaneFile) -> Result<()> {
    let path = admin_dir.join(LANE_FILE);
    if let Some(seen) = read_seen(&path)?
        && let Some(schema) = seen.schema
        && schema > SUPPORTED_SCHEMA
    {
        bail!(
            "{} has schema {schema}, newer than the schema {SUPPORTED_SCHEMA} this orca-term \
             supports; upgrade orca-term to change this lane",
            path.display()
        );
    }
    std::fs::create_dir_all(admin_dir)
        .with_context(|| format!("could not create {}", admin_dir.display()))?;
    let bytes = serde_json::to_vec(file)?;
    write_atomically(&path, &bytes)
}

fn read_seen(path: &Path) -> Result<Option<Seen>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("could not parse {}", path.display()))
            .map(Some),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
    }
}

/// Writes a temp file beside `path` and renames it over `path`, so readers see all or nothing.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;

    use super::*;

    #[test]
    fn writes_schema_1_with_no_agent() {
        let dir = TempDir::new().unwrap();
        let admin = dir.path().join("orca-term");
        write(&admin, &LaneFile::fresh()).unwrap();
        assert_eq!(
            std::fs::read_to_string(admin.join(LANE_FILE)).unwrap(),
            r#"{"schema":1,"agent":null}"#
        );
        assert_eq!(read(&admin).unwrap(), State::NoAgent);
        let leftovers: Vec<_> = std::fs::read_dir(&admin).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "the temp file is renamed away");
    }

    #[test]
    fn refuses_to_write_over_a_newer_schema() {
        let dir = TempDir::new().unwrap();
        let newer = r#"{"schema":2,"agent":null,"holder":{"pid":1}}"#;
        std::fs::write(dir.path().join(LANE_FILE), newer).unwrap();
        let err = write(dir.path(), &LaneFile::fresh()).unwrap_err();
        assert!(err.to_string().contains("upgrade orca-term"), "{err}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(LANE_FILE)).unwrap(),
            newer
        );
    }

    #[test]
    fn overwrites_a_file_of_the_supported_schema() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(LANE_FILE), r#"{"schema":1,"agent":null}"#).unwrap();
        write(dir.path(), &LaneFile::fresh()).unwrap();
    }

    #[test]
    fn reads_any_schema_and_ignores_unknown_fields() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(LANE_FILE),
            r#"{"schema":7,"agent":null,"future":[1,2]}"#,
        )
        .unwrap();
        assert_eq!(read(dir.path()).unwrap(), State::NoAgent);
    }

    #[test]
    fn a_missing_file_is_a_lane_with_no_agent() {
        let dir = TempDir::new().unwrap();
        assert_eq!(read(&dir.path().join("absent")).unwrap(), State::NoAgent);
        assert!(!dir.path().join("absent").exists(), "reading never writes");
    }
}
