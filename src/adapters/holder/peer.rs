//! Adapter: who is at the other end of a holder's socket (#61). On Linux the holder serves a
//! client only from its own network namespace: a command in an agent's sandbox runs in a
//! namespace of its own, and could otherwise connect to any worktree's `agent.sock` and type into
//! that agent. The client's pid comes from the socket (`SO_PEERCRED`), its namespace from
//! `/proc/<pid>/ns/net`; the rule itself is `protocol::admit`.
//!
//! macOS needs no check: its sandbox blocks unix sockets, so every client is served.

use std::os::unix::net::UnixStream;

#[cfg(target_os = "linux")]
use crate::adapters::fs;
#[cfg(target_os = "linux")]
use crate::domain::protocol::{self, NsId};

/// Whether the holder serves the client at the other end of `stream`.
#[cfg(target_os = "linux")]
pub(crate) fn admitted(stream: &UnixStream) -> bool {
    protocol::admit(own_namespace(), peer_namespace(stream))
}

/// Whether the holder serves the client at the other end of `stream`: always, off Linux.
#[cfg(not(target_os = "linux"))]
pub(crate) fn admitted(_stream: &UnixStream) -> bool {
    true
}

/// The holder's own network namespace.
#[cfg(target_os = "linux")]
fn own_namespace() -> Option<NsId> {
    namespace_at("/proc/self/ns/net")
}

/// The network namespace of the process that connected `stream`, as the kernel recorded it when
/// it connected.
#[cfg(target_os = "linux")]
fn peer_namespace(stream: &UnixStream) -> Option<NsId> {
    let peer = rustix::net::sockopt::socket_peercred(stream).ok()?;
    let pid = peer.pid.as_raw_nonzero();
    namespace_at(&format!("/proc/{pid}/ns/net"))
}

#[cfg(target_os = "linux")]
fn namespace_at(link: &str) -> Option<NsId> {
    NsId::parse(&fs::link_text(std::path::Path::new(link))?)
}
