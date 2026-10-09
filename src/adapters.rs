//! Adapter: the only modules that touch the outside world. Each one opts out of Clippy's
//! `disallowed-methods` and `disallowed-types` (`clippy.toml`) for the I/O it owns.

pub(crate) mod env;
pub(crate) mod fs;
pub(crate) mod git;
