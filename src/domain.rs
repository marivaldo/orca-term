//! Domain: the rules of the core, as pure code. Nothing here touches the file system, starts a
//! process, reads the environment or prints: functions take data and return data or a decision.

pub(crate) mod branch;
pub(crate) mod config;
pub(crate) mod contract;
pub(crate) mod fleet;
pub(crate) mod include;
pub(crate) mod porcelain;
pub(crate) mod state;
pub(crate) mod worktree;
