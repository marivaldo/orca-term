//! Domain: the approval of what a committed `orca-term.yaml` asks for (#58): its setup script,
//! which runs with the person's permissions outside any sandbox, and its `sandbox.writable` list,
//! which widens claude's sandbox. A committed file, the primary checkout's or a worktree's, was
//! written by whoever committed it, so its script runs, and its list is used, only once the person
//! has approved that exact text in this repository. What `.git/orca-term.yaml` or the global
//! config file says was written by the person and needs no approval.
//!
//! An approval is the sha256 of the script's bytes, or of the list's paths as written joined by
//! `\n`, recorded per repository in the git common dir as `orca-term-trust.json`:
//! `{"schema": 1, "approved": [{"kind", "sha256", "source", "approved_at"}]}`, where `kind` is
//! `setup` or `sandbox` (a missing `kind` reads as `setup`), `source` is the file it came from,
//! relative to its checkout (informational), and `approved_at` the time in milliseconds since the
//! Unix epoch. Any change needs a new approval. The file is outside every worktree, and claude's
//! sandbox denies writes to it.
//!
//! Like every state file it carries `schema`: reading takes the fields it knows from any schema,
//! and a schema newer than this core supports is never written over.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::domain::error::TypedError;
use crate::domain::state::ADMIN_DIR_NAME;

/// The approvals' file, in the git common dir.
pub(crate) const TRUST_FILE: &str = "orca-term-trust.json";

/// The lock the commands take, waiting briefly, while they change the approvals' file.
const TRUST_LOCK_FILE: &str = "trust.lock";

/// The approvals' file of the repository whose git common dir is `common`.
pub(crate) fn trust_file(common: &Path) -> PathBuf {
    common.join(TRUST_FILE)
}

/// The lock taken while the approvals' file of the repository whose git common dir is `common`
/// changes. It sits in the `orca-term` directory of the git common dir, where claude's sandbox
/// denies writes, so an agent cannot hold it.
pub(crate) fn trust_lock(common: &Path) -> PathBuf {
    common.join(ADMIN_DIR_NAME).join(TRUST_LOCK_FILE)
}

/// The newest schema of `orca-term-trust.json` this core reads and writes.
pub(crate) const TRUST_SCHEMA: u64 = 1;

/// What the person answers to approve a script at the prompt; anything else refuses it.
const YES_ANSWERS: &[&str] = &["y", "yes"];

/// What an approval covers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    /// A setup script (`scripts.setup`).
    #[default]
    Setup,
    /// The sandbox's extra writable paths (`sandbox.writable`).
    Sandbox,
}

impl Kind {
    /// How the error `untrusted` names it in `--json`, as `what`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::Sandbox => "sandbox",
        }
    }
}

/// One approved script or list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Approval {
    /// What it covers; an approval written before the kind existed is a setup script's.
    #[serde(default)]
    pub(crate) kind: Kind,
    /// The sha256 of the script's text or of the list, in lowercase hex.
    pub(crate) sha256: String,
    /// The file the script came from, relative to its checkout, for a person reading the file.
    #[serde(default)]
    pub(crate) source: String,
    /// When it was approved, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub(crate) approved_at: u64,
}

/// Every approval of a repository, as `orca-term-trust.json` records them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trust {
    #[serde(default)]
    schema: Option<u64>,
    #[serde(default)]
    approved: Vec<Approval>,
}

impl Trust {
    /// Whether the script or list of `kind` whose sha256 is `sha256` is approved.
    pub(crate) fn is_approved(&self, kind: Kind, sha256: &str) -> bool {
        self.approved
            .iter()
            .any(|approval| approval.kind == kind && approval.sha256 == sha256)
    }

    /// Records the approval of the script or list of `kind` whose sha256 is `sha256`, from
    /// `source`, at `now` (in milliseconds since the Unix epoch). One approved already is left as
    /// it was.
    pub(crate) fn approve(&mut self, kind: Kind, sha256: &str, source: &str, now: u64) {
        if self.is_approved(kind, sha256) {
            return;
        }
        self.approved.push(Approval {
            kind,
            sha256: sha256.to_owned(),
            source: source.to_owned(),
            approved_at: now,
        });
    }

    /// The bytes written to `orca-term-trust.json`, with this core's schema. Refuses a record read
    /// from a newer schema, so an older core never clobbers what a newer one wrote.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        if let Some(schema) = self.schema
            && schema > TRUST_SCHEMA
        {
            bail!(
                "{TRUST_FILE} has schema {schema}, newer than the schema {TRUST_SCHEMA} this \
                 orca-term supports; upgrade orca-term to approve it"
            );
        }
        let file = Self {
            schema: Some(TRUST_SCHEMA),
            approved: self.approved.clone(),
        };
        Ok(serde_json::to_vec(&file)?)
    }
}

/// The approvals in `orca-term-trust.json`, whose content is `bytes` (`None` when it does not
/// exist: nothing approved yet). A file that is not a record is refused, naming the file, so it is
/// never written over.
pub(crate) fn read(bytes: Option<&[u8]>) -> Result<Trust> {
    let Some(bytes) = bytes else {
        return Ok(Trust::default());
    };
    match serde_json::from_slice(bytes) {
        Ok(trust) => Ok(trust),
        Err(err) => bail!("could not parse {TRUST_FILE}: {err}; fix or remove it"),
    }
}

/// The sha256 of `script`'s exact bytes, in lowercase hex: what an approval records.
pub(crate) fn sha256(script: &str) -> String {
    let digest = Sha256::digest(script.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The sha256 of a `sandbox.writable` list: its paths as written, in order, joined by `\n`.
pub(crate) fn writable_sha256(paths: &[String]) -> String {
    sha256(&paths.join("\n"))
}

/// Whether `answer`, a line the person typed at the prompt, approves it: `y` or `yes`,
/// whatever the case and the blanks around it. Anything else, an empty line included, refuses it.
pub(crate) fn is_yes(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    YES_ANSWERS.contains(&answer.as_str())
}

/// What a committed `orca-term.yaml` asks for that needs the person's approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Subject {
    /// A setup script, its text exactly as the config file holds it.
    Setup { script: String },
    /// The sandbox's extra writable paths, as the config file writes them, in order.
    Sandbox { writable: Vec<String> },
}

impl Subject {
    /// What an approval of it covers.
    pub(crate) fn kind(&self) -> Kind {
        match self {
            Self::Setup { .. } => Kind::Setup,
            Self::Sandbox { .. } => Kind::Sandbox,
        }
    }

    /// The sha256 an approval of it records.
    pub(crate) fn sha256(&self) -> String {
        match self {
            Self::Setup { script } => sha256(script),
            Self::Sandbox { writable } => writable_sha256(writable),
        }
    }
}

/// What a committed `orca-term.yaml` asks for that the person has not approved yet, as the prompt,
/// the error `untrusted` and the client show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unapproved {
    /// The worktree it is for, by name.
    pub(crate) name: String,
    /// The script or the list.
    pub(crate) subject: Subject,
    /// The committed file it comes from, absolute.
    pub(crate) file: PathBuf,
}

impl Unapproved {
    /// The typed error `untrusted` for it: the script did not run, or the agent did not start.
    pub(crate) fn refused(&self) -> TypedError {
        TypedError::Untrusted {
            name: self.name.clone(),
            subject: self.subject.clone(),
            file: self.file.clone(),
        }
    }
}

/// How a command may get the person's approval of what a committed file asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consent {
    /// The person approved it beforehand (`--approve-setup`, `--approve-sandbox`).
    Given,
    /// The person is at a terminal and can be asked.
    Ask,
    /// Nobody can be asked: no terminal, or `--json`.
    Unavailable,
}

/// What a command does with a setup script, or a writable list, before using it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Run it: it needs no approval, or it is approved already.
    Run,
    /// Record its approval, then run it.
    ApproveAndRun,
    /// Ask the person; yes records the approval and runs it, anything else refuses it.
    Ask,
    /// Do not run it: it stays not approved.
    Refuse,
}

/// What to do with a setup script or a writable list that comes from a committed file when
/// `committed` is true, and that is approved already when `approved` is true, given the person's
/// `consent`.
pub(crate) fn gate(committed: bool, approved: bool, consent: Consent) -> Gate {
    if !committed || approved {
        return Gate::Run;
    }
    match consent {
        Consent::Given => Gate::ApproveAndRun,
        Consent::Ask => Gate::Ask,
        Consent::Unavailable => Gate::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_the_sha256_of_the_exact_bytes() {
        assert_eq!(
            sha256(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(
            sha256("make\n"),
            sha256("make"),
            "a trailing newline counts"
        );
    }

    #[test]
    fn the_approvals_live_in_the_git_common_dir() {
        let common = Path::new("/r/.git");
        assert_eq!(
            trust_file(common),
            PathBuf::from("/r/.git/orca-term-trust.json")
        );
        assert_eq!(
            trust_lock(common),
            PathBuf::from("/r/.git/orca-term/trust.lock")
        );
    }

    #[test]
    fn a_missing_file_approves_nothing() {
        let trust = read(None).unwrap();
        assert!(!trust.is_approved(Kind::Setup, &sha256("make")));
    }

    #[test]
    fn an_approval_is_written_with_the_schema_and_read_back() {
        let mut trust = Trust::default();
        let hash = sha256("make\n");
        trust.approve(Kind::Setup, &hash, "orca-term.yaml", 1_700_000_000_000);
        trust.approve(Kind::Setup, &hash, "orca-term.yaml", 1_800_000_000_000);
        let bytes = trust.to_bytes().unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            format!(
                r#"{{"schema":1,"approved":[{{"kind":"setup","sha256":"{hash}","source":"orca-term.yaml","approved_at":1700000000000}}]}}"#
            ),
            "approving twice records it once, at the first time"
        );
        let read_back = read(Some(&bytes)).unwrap();
        assert!(read_back.is_approved(Kind::Setup, &hash));
        assert!(
            !read_back.is_approved(Kind::Sandbox, &hash),
            "one approval per kind"
        );
        assert!(!read_back.is_approved(Kind::Setup, &sha256("make")));
    }

    #[test]
    fn a_newer_schema_is_read_but_never_written_over() {
        let hash = sha256("make");
        let bytes =
            format!(r#"{{"schema":2,"approved":[{{"sha256":"{hash}","extra":true}}],"other":1}}"#);
        let mut trust = read(Some(bytes.as_bytes())).unwrap();
        assert!(
            trust.is_approved(Kind::Setup, &hash),
            "no kind reads as setup"
        );
        trust.approve(Kind::Setup, &sha256("other"), "orca-term.yaml", 1);
        let err = trust.to_bytes().unwrap_err().to_string();
        assert!(err.contains("schema 2, newer than the schema 1"), "{err}");
    }

    #[test]
    fn a_writable_list_is_hashed_as_its_paths_joined_by_newlines() {
        let list = vec!["~/.cache/pnpm".to_owned(), "../shared".to_owned()];
        assert_eq!(writable_sha256(&list), sha256("~/.cache/pnpm\n../shared"));
        let reordered = vec!["../shared".to_owned(), "~/.cache/pnpm".to_owned()];
        assert_ne!(
            writable_sha256(&list),
            writable_sha256(&reordered),
            "order counts"
        );
        let subject = Subject::Sandbox { writable: list };
        assert_eq!(subject.kind(), Kind::Sandbox);
        assert_eq!(subject.sha256(), sha256("~/.cache/pnpm\n../shared"));
        assert_eq!(Kind::Sandbox.name(), "sandbox");
        assert_eq!(Kind::Setup.name(), "setup");
    }

    #[test]
    fn a_corrupt_file_is_refused_by_name() {
        let err = read(Some(b"{not json")).unwrap_err().to_string();
        assert!(
            err.starts_with("could not parse orca-term-trust.json"),
            "{err}"
        );
    }

    #[test]
    fn only_y_or_yes_approves() {
        for yes in ["y", "Y", "yes", "YES", " Yes \n", "y\r\n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "y es", "sure"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[test]
    fn only_an_unapproved_committed_script_is_gated() {
        for consent in [Consent::Given, Consent::Ask, Consent::Unavailable] {
            assert_eq!(gate(false, false, consent), Gate::Run, "not committed");
            assert_eq!(gate(true, true, consent), Gate::Run, "approved already");
        }
        assert_eq!(gate(true, false, Consent::Given), Gate::ApproveAndRun);
        assert_eq!(gate(true, false, Consent::Ask), Gate::Ask);
        assert_eq!(gate(true, false, Consent::Unavailable), Gate::Refuse);
    }
}
