//! Adapter: who is at the other end of a holder's socket (#61, #35). On Linux the holder serves a
//! client only from its own network and user namespaces: a command in claude's sandbox runs in a
//! network namespace of its own, and one in opencode's in a user namespace of its own, and either
//! could otherwise connect to any worktree's `agent.sock` and type into that agent. The client's
//! pid comes from the socket (`SO_PEERCRED`), its namespaces from `/proc/<pid>/ns/net` and
//! `/proc/<pid>/ns/user`; the rule itself is `protocol::admit`.
//!
//! macOS needs no check: both sandboxes block unix sockets, so every client is served.

use std::os::unix::net::UnixStream;

#[cfg(target_os = "linux")]
use crate::adapters::fs;
#[cfg(target_os = "linux")]
use crate::domain::protocol::{self, Namespaces, NsId, NsKind};

/// Whether the holder serves the client at the other end of `stream`.
#[cfg(target_os = "linux")]
pub(crate) fn admitted(stream: &UnixStream) -> bool {
    protocol::admit(namespaces_of("self"), peer_namespaces(stream))
}

/// Whether the holder serves the client at the other end of `stream`: always, off Linux.
#[cfg(not(target_os = "linux"))]
pub(crate) fn admitted(_stream: &UnixStream) -> bool {
    true
}

/// The namespaces of the process that connected `stream`, as the kernel recorded it when it
/// connected; none can be read when its pid cannot.
#[cfg(target_os = "linux")]
fn peer_namespaces(stream: &UnixStream) -> Namespaces {
    match rustix::net::sockopt::socket_peercred(stream) {
        Ok(peer) => namespaces_of(&peer.pid.as_raw_nonzero().to_string()),
        Err(_) => Namespaces {
            net: None,
            user: None,
        },
    }
}

/// The network and user namespaces of the process `/proc/<process>` names (`self` for the
/// holder).
#[cfg(target_os = "linux")]
fn namespaces_of(process: &str) -> Namespaces {
    Namespaces {
        net: namespace_at(process, NsKind::Net),
        user: namespace_at(process, NsKind::User),
    }
}

#[cfg(target_os = "linux")]
fn namespace_at(process: &str, kind: NsKind) -> Option<NsId> {
    let link = format!("/proc/{process}/ns/{}", kind.name());
    NsId::parse(kind, &fs::link_text(std::path::Path::new(&link))?)
}
