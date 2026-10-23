//! Domain: the person's signing agent and the agent's sandbox (#105). A signing key in a file signs
//! inside both sandboxes, since reads are open; a key held only by an agent (ssh-agent, 1Password,
//! Secretive, a hardware key, gpg-agent) signs only through the agent's unix socket, which both
//! sandboxes deny. `sandbox.agent_socket: true` allows exactly that socket: the one
//! `SSH_AUTH_SOCK` names at launch, and gpg-agent's when git signs with openpgp. The adapters find
//! the sockets and the files; this module reads what git's config and gpgconf say, and what
//! `orca-term doctor` says about a key only an agent holds, and what `agent start` says it
//! allowed (#108).

use std::path::{Path, PathBuf};

use crate::domain::prerequisite::{Check, Status};

/// The program that names gpg-agent's socket, and its arguments for it.
pub(crate) const GPGCONF: &str = "gpgconf";
pub(crate) const GPG_SOCKET_ARGS: &[&str] = &["--list-dirs", "agent-socket"];

/// The git config keys the signing rules read.
pub(crate) const FORMAT_KEY: &str = "gpg.format";
pub(crate) const SIGNING_KEY_KEY: &str = "user.signingkey";

/// Whether git signs with openpgp, so through gpg-agent: `gpg.format` is `openpgp`, or unset,
/// which git reads as openpgp.
pub(crate) fn signs_with_openpgp(format: Option<&str>) -> bool {
    format.is_none_or(|format| format == "openpgp")
}

/// gpg-agent's socket, from what `gpgconf --list-dirs agent-socket` printed: its first line, when
/// it is an absolute path. `None` for one gpgconf percent-escaped (a `%` in it), which is never
/// guessed at.
pub(crate) fn gpg_socket(printed: &[u8]) -> Option<PathBuf> {
    let text = String::from_utf8_lossy(printed);
    let line = text.lines().next()?.trim();
    let path = Path::new(line);
    (path.is_absolute() && !line.contains('%')).then(|| path.to_owned())
}

/// Which signing agent a socket belongs to: the ssh agent `SSH_AUTH_SOCK` names, or gpg-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SocketKind {
    SshAgent,
    GpgAgent,
}

impl SocketKind {
    /// The kind's name in `--json`: `ssh-agent` or `gpg-agent`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::SshAgent => "ssh-agent",
            Self::GpgAgent => "gpg-agent",
        }
    }

    /// The line `agent start` prints when the sandbox reaches a socket of this kind. Never the
    /// socket's path, which is the value of `SSH_AUTH_SOCK`.
    pub(crate) fn reached_line(self) -> &'static str {
        match self {
            Self::SshAgent => "signing: the sandbox reaches your ssh agent",
            Self::GpgAgent => "signing: the sandbox reaches gpg-agent",
        }
    }
}

/// What `agent start` did with `sandbox.agent_socket` on (#108): the kinds of socket the sandbox
/// reaches, in the order they were found, and whether gpg-agent's was looked for (git signs with
/// openpgp).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SigningReach {
    pub(crate) kinds: Vec<SocketKind>,
    pub(crate) looked_for_gpg: bool,
}

impl SigningReach {
    /// The lines `agent start` prints: one per kind reached, or, when none was, one warning that
    /// names what was looked for. The gpgconf part appears only when gpg-agent was looked for.
    pub(crate) fn lines(&self) -> Vec<String> {
        if !self.kinds.is_empty() {
            return self
                .kinds
                .iter()
                .map(|kind| kind.reached_line().to_owned())
                .collect();
        }
        let gpg = if self.looked_for_gpg {
            "; gpgconf names no gpg-agent socket"
        } else {
            ""
        };
        vec![format!(
            "signing: sandbox.agent_socket is on, but no signing agent socket was found \
             (SSH_AUTH_SOCK is unset or not a socket{gpg})"
        )]
    }
}

/// Whether the socket at `path` lies inside the repository's git common dir `common`, where
/// orca-term's own sockets (the holders') live: such a socket is never allowed, or one worktree's
/// agent could talk to another worktree's holder. Both paths already resolved to their real paths.
pub(crate) fn is_inside_git_dir(path: &Path, common: &Path) -> bool {
    path.starts_with(common)
}

/// The ssh key git signs with, as its config names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SshSigningKey {
    /// git does not sign with ssh, or names no key this check can judge.
    NotJudged,
    /// The key is written in the config itself (`key::...`, or a bare `ssh-...`): only an agent
    /// can hold its private half.
    Literal,
    /// A public key file: its private half is expected at this path, the same without `.pub`.
    PublicFile { private: PathBuf },
}

/// The ssh key git signs with, from `gpg.format` (`format`) and `user.signingkey` (`key`), a
/// leading `~/` expanded against `home`. Only an absolute `.pub` path, once expanded, names a
/// public key file.
pub(crate) fn ssh_signing_key(
    format: Option<&str>,
    key: Option<&str>,
    home: Option<&Path>,
) -> SshSigningKey {
    let (Some("ssh"), Some(key)) = (format, key.map(str::trim)) else {
        return SshSigningKey::NotJudged;
    };
    if key.starts_with("key::") || key.starts_with("ssh-") {
        return SshSigningKey::Literal;
    }
    let path = match (key.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        (Some(_), None) => return SshSigningKey::NotJudged,
        (None, _) => PathBuf::from(key),
    };
    match path.to_str().and_then(|text| text.strip_suffix(".pub")) {
        Some(private) if path.is_absolute() && !private.ends_with('/') => {
            SshSigningKey::PublicFile {
                private: PathBuf::from(private),
            }
        }
        _ => SshSigningKey::NotJudged,
    }
}

/// What `orca-term doctor` says when git signs with an ssh key whose private half is not a
/// readable file, so only an agent can sign with it: information, naming the setting that lets a
/// worktree's agent reach it. Never the key nor its path.
pub(crate) fn agent_signing_check() -> Check {
    Check {
        name: "signing",
        status: Status::Info,
        found: None,
        need: None,
        message: "signing: git signs with an ssh key whose private half is not a readable file, \
                  so it signs through your ssh agent; a worktree's agent can reach it only with \
                  `sandbox.agent_socket: true` in your global config file or .git/orca-term.yaml"
            .to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_signs_with_openpgp_when_the_format_says_so_or_is_unset() {
        assert!(signs_with_openpgp(None));
        assert!(signs_with_openpgp(Some("openpgp")));
        assert!(!signs_with_openpgp(Some("ssh")));
        assert!(!signs_with_openpgp(Some("x509")));
    }

    #[test]
    fn gpg_agents_socket_is_the_first_line_gpgconf_prints_when_absolute() {
        assert_eq!(
            gpg_socket(b"/Users/me/.gnupg/S.gpg-agent\n"),
            Some(PathBuf::from("/Users/me/.gnupg/S.gpg-agent"))
        );
        assert_eq!(gpg_socket(b""), None);
        assert_eq!(gpg_socket(b"S.gpg-agent\n"), None);
        assert_eq!(gpg_socket(b"/a%3ab/S.gpg-agent\n"), None);
    }

    #[test]
    fn the_ssh_signing_key_is_judged_only_for_ssh_and_a_pub_path_or_a_literal_key() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            ssh_signing_key(Some("ssh"), Some("~/.ssh/id_ed25519.pub"), home),
            SshSigningKey::PublicFile {
                private: PathBuf::from("/home/me/.ssh/id_ed25519")
            }
        );
        assert_eq!(
            ssh_signing_key(Some("ssh"), Some("/k/agentonly.pub"), None),
            SshSigningKey::PublicFile {
                private: PathBuf::from("/k/agentonly")
            }
        );
        assert_eq!(
            ssh_signing_key(Some("ssh"), Some("key::ssh-ed25519 AAAA me"), home),
            SshSigningKey::Literal
        );
        assert_eq!(
            ssh_signing_key(Some("ssh"), Some("ssh-ed25519 AAAA me"), home),
            SshSigningKey::Literal
        );
        for (format, key) in [
            (None, Some("/k/a.pub")),
            (Some("openpgp"), Some("/k/a.pub")),
            (Some("ssh"), None),
            (Some("ssh"), Some("/k/id_ed25519")),
            (Some("ssh"), Some("relative.pub")),
            (Some("ssh"), Some("/k/.pub")),
        ] {
            assert_eq!(
                ssh_signing_key(format, key, home),
                SshSigningKey::NotJudged,
                "{format:?} {key:?}"
            );
        }
        assert_eq!(
            ssh_signing_key(Some("ssh"), Some("~/.ssh/a.pub"), None),
            SshSigningKey::NotJudged,
            "no HOME to expand against"
        );
    }

    #[test]
    fn socket_kinds_have_their_json_names() {
        assert_eq!(SocketKind::SshAgent.name(), "ssh-agent");
        assert_eq!(SocketKind::GpgAgent.name(), "gpg-agent");
    }

    #[test]
    fn agent_start_says_one_line_per_kind_reached() {
        let reach = SigningReach {
            kinds: vec![SocketKind::SshAgent, SocketKind::GpgAgent],
            looked_for_gpg: true,
        };
        assert_eq!(
            reach.lines(),
            [
                "signing: the sandbox reaches your ssh agent",
                "signing: the sandbox reaches gpg-agent",
            ]
        );
    }

    #[test]
    fn agent_start_warns_when_no_socket_was_found_naming_gpgconf_only_for_openpgp() {
        let mut reach = SigningReach {
            kinds: Vec::new(),
            looked_for_gpg: true,
        };
        assert_eq!(
            reach.lines(),
            [
                "signing: sandbox.agent_socket is on, but no signing agent socket was found \
                 (SSH_AUTH_SOCK is unset or not a socket; gpgconf names no gpg-agent socket)"
            ]
        );
        reach.looked_for_gpg = false;
        assert_eq!(
            reach.lines(),
            [
                "signing: sandbox.agent_socket is on, but no signing agent socket was found \
                 (SSH_AUTH_SOCK is unset or not a socket)"
            ]
        );
    }

    #[test]
    fn a_socket_is_inside_the_git_common_dir_only_when_under_it() {
        let common = Path::new("/r/.git");
        assert!(is_inside_git_dir(
            Path::new("/r/.git/orca-term/h.sock"),
            common
        ));
        assert!(is_inside_git_dir(
            Path::new("/r/.git/worktrees/fix/orca-term/holder.sock"),
            common
        ));
        assert!(!is_inside_git_dir(Path::new("/r/.gitx/agent.sock"), common));
        assert!(!is_inside_git_dir(Path::new("/tmp/agent.sock"), common));
    }

    #[test]
    fn the_doctor_names_the_setting_and_never_the_key() {
        let check = agent_signing_check();
        assert_eq!(check.name, "signing");
        assert_eq!(check.status, Status::Info);
        assert!(!check.status.refuses());
        assert!(check.message.contains("`sandbox.agent_socket: true`"));
        assert!(!check.message.contains(".pub"));
    }
}
