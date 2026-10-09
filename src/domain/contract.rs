//! Domain: the integers that keep the core and its clients in step (ADR 0008).

/// Carried by every `--json` output. Rises only when the JSON contract breaks.
pub(crate) const CONTRACT: u32 = 1;

/// The core's own version, carried next to the contract.
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
