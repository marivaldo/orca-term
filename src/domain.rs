//! Domain: the rules of the core, as pure code. Nothing here touches the file system, starts a
//! process, reads the environment or prints: functions take data and return data or a decision.

pub(crate) mod agent;
pub(crate) mod blame;
pub(crate) mod branch;
pub(crate) mod config;
pub(crate) mod contract;
pub(crate) mod detail;
pub(crate) mod error;
pub(crate) mod fleet;
pub(crate) mod history;
pub(crate) mod hook;
pub(crate) mod include;
pub(crate) mod launch;
pub(crate) mod note;
pub(crate) mod nvim;
pub(crate) mod operation;
pub(crate) mod porcelain;
pub(crate) mod pr;
pub(crate) mod prerequisite;
pub(crate) mod protocol;
pub(crate) mod review;
pub(crate) mod sandbox;
pub(crate) mod seen;
pub(crate) mod session;
pub(crate) mod setup;
pub(crate) mod snapshot;
pub(crate) mod state;
pub(crate) mod trust;
pub(crate) mod turn;
pub(crate) mod worktree;
