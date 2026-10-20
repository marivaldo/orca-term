//! Domain: the configuration, YAML files layered key by key.
//!
//! Precedence, highest first: the uncommitted override `orca-term.yaml` in the git common dir, the
//! committed `orca-term.yaml` at the root of a checkout, the global
//! `$XDG_CONFIG_HOME/orca-term/config.yaml` (or `~/.config/orca-term/config.yaml`), then the
//! built-in default. Every resolved key remembers which source won and which it overrode, so the
//! precedence can be printed.
//!
//! Which checkout supplies the committed file depends on the key, as in Orca: `base` comes from
//! the primary checkout, since it decides where a worktree goes before the worktree exists, and
//! `scripts.setup` and `sandbox.writable` come from the worktree itself, from the revision
//! checked out there. A setup script from a committed file runs only once the person approved it
//! (`domain/trust.rs`); one from the override or the global file is the person's own.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_norway::Value;

use crate::domain::fleet::PrimaryCheckout;

/// The config file's name, both committed and in the git common dir.
pub(crate) const FILE_NAME: &str = "orca-term.yaml";

/// Every top-level key this version understands.
const KEYS: &[&str] = &["base", "scripts", "sandbox"];

/// The built-in default for `base`, the directory worktrees live under.
const DEFAULT_BASE: &str = "~/orca-term/worktrees";

/// The process environment config resolution depends on, as values: the env adapter reads them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Env {
    pub(crate) home: Option<PathBuf>,
    pub(crate) xdg_config_home: Option<PathBuf>,
}

impl Env {
    /// The global config file. A relative `XDG_CONFIG_HOME` is ignored, as the XDG spec says.
    fn global_file(&self) -> Option<PathBuf> {
        let dir = self
            .xdg_config_home
            .clone()
            .filter(|p| p.is_absolute())
            .or_else(|| self.home.as_ref().map(|h| h.join(".config")))?;
        Some(dir.join("orca-term").join("config.yaml"))
    }
}

/// One resolved key: its value, the source that won and the lower sources it overrode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Setting<T> {
    pub(crate) value: T,
    /// The winning file's label, or `None` for the built-in default.
    pub(crate) source: Option<String>,
    /// Labels of the lower-precedence files that also set the key, highest first.
    pub(crate) overridden: Vec<String>,
}

impl<T> Setting<T> {
    /// Where the value came from, as `built-in default`, `from <file>` or
    /// `from <file>, overriding <files>`.
    pub(crate) fn origin(&self) -> String {
        match &self.source {
            None => "built-in default".to_owned(),
            Some(source) if self.overridden.is_empty() => format!("from {source}"),
            Some(source) => format!("from {source}, overriding {}", self.overridden.join(", ")),
        }
    }
}

impl fmt::Display for Setting<PathBuf> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.value.display(), self.origin())
    }
}

/// Where a config file may be, how the precedence line names it and what relative paths in it
/// resolve against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    label: String,
    anchor: PathBuf,
    /// Whether it is a committed `orca-term.yaml`, written by whoever committed it rather than
    /// by the person: its setup script needs the person's approval (`domain/trust.rs`).
    committed: bool,
}

/// One config file that exists, parsed.
#[derive(Debug)]
pub(crate) struct Layer {
    /// Where the file is.
    path: PathBuf,
    /// How the precedence line names it.
    label: String,
    /// What relative paths in it resolve against.
    anchor: PathBuf,
    /// Whether it is a committed `orca-term.yaml`.
    committed: bool,
    raw: Raw,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    base: Option<String>,
    scripts: Option<Scripts>,
    sandbox: Option<Sandbox>,
}

/// The `scripts` mapping, the same shape as Orca's.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scripts {
    setup: Option<String>,
}

/// The `sandbox` mapping: the only configurable part of the sandbox policy (ADR 0007), the extra
/// paths the agent may write to, such as package-manager caches.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sandbox {
    writable: Option<Vec<String>>,
}

/// Every config file of the repository whose primary checkout is `primary` and whose git common
/// dir is `common_dir`, highest precedence first, with the committed file read from the root of
/// `checkout` (the primary checkout or a worktree). Some of them may not exist.
pub(crate) fn sources(
    primary: &PrimaryCheckout,
    checkout: &Path,
    common_dir: &Path,
    env: &Env,
) -> Vec<Source> {
    let root = &primary.path;
    let local = common_dir.join(FILE_NAME);
    let mut sources = vec![
        Source {
            label: label_within(&local, root),
            path: local,
            anchor: root.clone(),
            committed: false,
        },
        Source {
            path: checkout.join(FILE_NAME),
            label: FILE_NAME.to_owned(),
            anchor: checkout.to_owned(),
            committed: true,
        },
    ];
    if let Some(global) = env.global_file() {
        sources.push(Source {
            label: global.display().to_string(),
            anchor: global.parent().map(Path::to_owned).unwrap_or_default(),
            path: global,
            committed: false,
        });
    }
    sources
}

impl Layer {
    /// Parses the text of the config file `source`. Malformed YAML and unknown keys are errors
    /// naming the file.
    pub(crate) fn parse(source: Source, text: &str) -> Result<Self> {
        let raw = parse(text)
            .with_context(|| format!("invalid config file {}", source.path.display()))?;
        Ok(Self {
            path: source.path,
            label: source.label,
            anchor: source.anchor,
            committed: source.committed,
            raw,
        })
    }
}

/// The directory worktrees live under, as `<base>/<repo>/<name>`: `base` resolved from `layers`,
/// highest precedence first, over the built-in default.
pub(crate) fn resolve_base(layers: &[Layer], env: &Env) -> Result<Setting<PathBuf>> {
    let home = env.home.as_deref();
    let mut setters = layers
        .iter()
        .filter_map(|l| l.raw.base.as_deref().map(|v| (l, v)));
    Ok(match setters.next() {
        Some((winner, value)) => Setting {
            value: expand(value, &winner.anchor, home)
                .with_context(|| format!("{}: invalid `base`", winner.label))?,
            source: Some(winner.label.clone()),
            overridden: setters.map(|(l, _)| l.label.clone()).collect(),
        },
        None => Setting {
            value: expand(DEFAULT_BASE, Path::new("/"), home).context(
                "cannot place worktrees under the built-in default; set `base` in orca-term.yaml",
            )?,
            source: None,
            overridden: Vec::new(),
        },
    })
}

/// The setup script: `scripts.setup` resolved from `layers`, highest precedence first. `None`
/// when no layer sets it, or when the winning layer sets it empty, which turns off a script a
/// lower layer sets.
pub(crate) fn resolve_setup(layers: &[Layer]) -> Option<Setting<String>> {
    let mut setters = setup_setters(layers);
    let (winner, script) = setters.next()?;
    if script.trim().is_empty() {
        return None;
    }
    Some(Setting {
        value: script.to_owned(),
        source: Some(winner.label.clone()),
        overridden: setters.map(|(l, _)| l.label.clone()).collect(),
    })
}

/// The committed `orca-term.yaml` the setup script comes from, when it comes from one: the file
/// whose layer wins [`resolve_setup`]. `None` when there is no script, or when it comes from the
/// uncommitted override or the global config file, which the person wrote and which need no
/// approval.
pub(crate) fn committed_setup_file(layers: &[Layer]) -> Option<&Path> {
    let (winner, script) = setup_setters(layers).next()?;
    (winner.committed && !script.trim().is_empty()).then_some(winner.path.as_path())
}

/// The layers that set `scripts.setup`, highest precedence first, each with its script.
fn setup_setters(layers: &[Layer]) -> impl Iterator<Item = (&Layer, &str)> {
    layers.iter().filter_map(|l| {
        let script = l.raw.scripts.as_ref()?.setup.as_deref()?;
        Some((l, script))
    })
}

/// The extra writable paths of the sandbox: `sandbox.writable` resolved from `layers`, highest
/// precedence first, each path expanded (`~`, relative to the file) and made absolute. The highest
/// layer that sets the list replaces the lower ones, it never merges with them. Empty, from the
/// built-in default, when no layer sets it.
pub(crate) fn resolve_writable(layers: &[Layer], env: &Env) -> Result<Setting<Vec<PathBuf>>> {
    let home = env.home.as_deref();
    let mut setters = layers.iter().filter_map(|l| {
        let paths = l.raw.sandbox.as_ref()?.writable.as_deref()?;
        Some((l, paths))
    });
    let Some((winner, paths)) = setters.next() else {
        return Ok(Setting {
            value: Vec::new(),
            source: None,
            overridden: Vec::new(),
        });
    };
    let value = paths
        .iter()
        .map(|raw| expand(raw, &winner.anchor, home))
        .collect::<Result<Vec<_>>>()
        .with_context(|| format!("{}: invalid `sandbox.writable`", winner.label))?;
    Ok(Setting {
        value,
        source: Some(winner.label.clone()),
        overridden: setters.map(|(l, _)| l.label.clone()).collect(),
    })
}

fn parse(text: &str) -> Result<Raw> {
    let value: Value = serde_norway::from_str(text).context("not valid YAML")?;
    let map = match value {
        Value::Null => return Ok(Raw::default()),
        Value::Mapping(map) => map,
        _ => bail!("expected a mapping of keys to values"),
    };
    for key in map.keys() {
        let Some(key) = key.as_str() else {
            bail!("keys must be strings, found {key:?}");
        };
        if !KEYS.contains(&key) {
            bail!("unknown key `{key}` (known keys: {})", KEYS.join(", "));
        }
    }
    serde_norway::from_value(Value::Mapping(map)).context("invalid value")
}

/// Expands `~` against `home` and resolves a relative path against `anchor`, lexically.
fn expand(raw: &str, anchor: &Path, home: Option<&Path>) -> Result<PathBuf> {
    if raw.trim().is_empty() {
        bail!("the path is empty");
    }
    let path = if raw == "~" || raw.starts_with("~/") {
        let Some(home) = home else {
            bail!("`{raw}` needs HOME, which is not set");
        };
        home.join(raw.trim_start_matches('~').trim_start_matches('/'))
    } else {
        anchor.join(raw)
    };
    Ok(normalize(&path))
}

/// Removes `.` components and folds `..` into its parent, without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `path` relative to `root` when it lies inside it, otherwise the full path.
fn label_within(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(label: &str, anchor: &str, base: Option<&str>) -> Layer {
        Layer {
            path: PathBuf::from(anchor).join(FILE_NAME),
            label: label.to_owned(),
            anchor: PathBuf::from(anchor),
            committed: label == FILE_NAME,
            raw: Raw {
                base: base.map(str::to_owned),
                scripts: None,
                sandbox: None,
            },
        }
    }

    fn setup_layer(label: &str, setup: Option<&str>) -> Layer {
        Layer {
            path: PathBuf::from("/repo").join(label),
            label: label.to_owned(),
            anchor: PathBuf::from("/repo"),
            committed: label == FILE_NAME,
            raw: Raw {
                base: None,
                scripts: Some(Scripts {
                    setup: setup.map(str::to_owned),
                }),
                sandbox: None,
            },
        }
    }

    fn writable_layer(label: &str, anchor: &str, writable: &[&str]) -> Layer {
        Layer {
            path: PathBuf::from(anchor).join(FILE_NAME),
            label: label.to_owned(),
            anchor: PathBuf::from(anchor),
            committed: label == FILE_NAME,
            raw: Raw {
                base: None,
                scripts: None,
                sandbox: Some(Sandbox {
                    writable: Some(writable.iter().map(|p| (*p).to_owned()).collect()),
                }),
            },
        }
    }

    fn primary() -> PrimaryCheckout {
        PrimaryCheckout {
            path: PathBuf::from("/repo"),
            branch: None,
        }
    }

    fn env() -> Env {
        Env {
            home: Some(PathBuf::from("/home/me")),
            xdg_config_home: None,
        }
    }

    #[test]
    fn the_built_in_default_lives_under_home() {
        let base = resolve_base(&[], &env()).unwrap();
        assert_eq!(base.value, PathBuf::from("/home/me/orca-term/worktrees"));
        assert_eq!(base.source, None);
        assert_eq!(
            base.to_string(),
            "/home/me/orca-term/worktrees (built-in default)"
        );
    }

    #[test]
    fn the_highest_layer_that_sets_a_key_wins_and_names_the_rest() {
        let layers = [
            layer(".git/orca-term.yaml", "/repo", Some("/l")),
            layer("orca-term.yaml", "/repo", None),
            layer("/g/config.yaml", "/g", Some("/g/worktrees")),
        ];
        let base = resolve_base(&layers, &env()).unwrap();
        assert_eq!(
            base.to_string(),
            "/l (from .git/orca-term.yaml, overriding /g/config.yaml)"
        );
    }

    #[test]
    fn relative_paths_resolve_against_the_layer_anchor() {
        let base = resolve_base(
            &[layer("orca-term.yaml", "/w/repo", Some("../worktrees"))],
            &env(),
        )
        .unwrap();
        assert_eq!(base.value, PathBuf::from("/w/worktrees"));
        assert_eq!(base.to_string(), "/w/worktrees (from orca-term.yaml)");
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = Path::new("/home/me");
        assert_eq!(
            expand("~", Path::new("/x"), Some(home)).unwrap(),
            PathBuf::from("/home/me")
        );
        assert_eq!(
            expand("~/a/./b", Path::new("/x"), Some(home)).unwrap(),
            PathBuf::from("/home/me/a/b")
        );
        assert_eq!(
            expand("~user/a", Path::new("/x"), Some(home)).unwrap(),
            PathBuf::from("/x/~user/a")
        );
        assert!(expand("~/a", Path::new("/x"), None).is_err());
        assert!(expand(" ", Path::new("/x"), Some(home)).is_err());
    }

    #[test]
    fn an_empty_or_comment_only_file_sets_nothing() {
        assert!(parse("").unwrap().base.is_none());
        assert!(parse("# nothing yet\n").unwrap().base.is_none());
    }

    #[test]
    fn unknown_keys_are_refused_by_name() {
        let err = parse("base: /l\nsharedDirectories: [node_modules]\n").unwrap_err();
        assert!(format!("{err:#}").contains("unknown key `sharedDirectories`"));
    }

    #[test]
    fn malformed_yaml_and_wrong_shapes_are_refused() {
        assert!(parse("base: [\n").is_err());
        assert!(parse("- base\n").is_err());
        assert!(parse("base: [a, b]\n").is_err());
    }

    #[test]
    fn the_sources_go_local_then_committed_then_global() {
        let sources = sources(
            &primary(),
            Path::new("/repo"),
            Path::new("/repo/.git"),
            &env(),
        );
        let labels: Vec<&str> = sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                ".git/orca-term.yaml",
                "orca-term.yaml",
                "/home/me/.config/orca-term/config.yaml"
            ]
        );
        assert_eq!(
            sources[2].anchor,
            PathBuf::from("/home/me/.config/orca-term")
        );
    }

    #[test]
    fn a_malformed_file_is_named_in_the_error() {
        let source = sources(
            &primary(),
            Path::new("/repo"),
            Path::new("/repo/.git"),
            &env(),
        )
        .remove(1);
        let err = Layer::parse(source, "bogus: 1\n").unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "invalid config file /repo/orca-term.yaml: unknown key `bogus` (known keys: base, \
             scripts, sandbox)"
        );
    }

    #[test]
    fn the_committed_file_comes_from_the_checkout_given() {
        let sources = sources(
            &primary(),
            Path::new("/w/repo/fix"),
            Path::new("/repo/.git"),
            &env(),
        );
        assert_eq!(sources[0].path, PathBuf::from("/repo/.git/orca-term.yaml"));
        assert_eq!(sources[0].label, ".git/orca-term.yaml");
        assert_eq!(sources[1].path, PathBuf::from("/w/repo/fix/orca-term.yaml"));
        assert_eq!(sources[1].anchor, PathBuf::from("/w/repo/fix"));
    }

    #[test]
    fn the_setup_script_is_parsed_from_the_scripts_mapping() {
        let raw = parse("scripts:\n  setup: |\n    pnpm install\n    echo done\n").unwrap();
        assert_eq!(
            raw.scripts.unwrap().setup.as_deref(),
            Some("pnpm install\necho done\n")
        );
        assert!(parse("scripts:\n").unwrap().scripts.is_none());
        let err = parse("scripts:\n  archive: rm -rf x\n").unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field `archive`"),
            "{err:#}"
        );
        assert!(parse("scripts:\n  setup: [a]\n").is_err());
        assert!(parse("scripts: make\n").is_err());
    }

    #[test]
    fn the_highest_layer_setting_the_setup_script_wins() {
        let setup = resolve_setup(&[
            setup_layer(".git/orca-term.yaml", Some("make local")),
            setup_layer("orca-term.yaml", Some("make")),
            layer("/g/config.yaml", "/g", Some("/g/w")),
        ])
        .unwrap();
        assert_eq!(setup.value, "make local");
        assert_eq!(
            setup.origin(),
            "from .git/orca-term.yaml, overriding orca-term.yaml"
        );
        let setup = resolve_setup(&[
            layer(".git/orca-term.yaml", "/repo", Some("/l")),
            setup_layer("orca-term.yaml", Some("make")),
        ])
        .unwrap();
        assert_eq!(setup.origin(), "from orca-term.yaml");
        assert_eq!(resolve_setup(&[layer("orca-term.yaml", "/r", None)]), None);
        assert_eq!(resolve_setup(&[]), None);
    }

    #[test]
    fn an_empty_setup_script_is_no_script_and_turns_off_a_lower_one() {
        assert_eq!(
            resolve_setup(&[setup_layer("orca-term.yaml", Some("  \n"))]),
            None
        );
        assert_eq!(
            resolve_setup(&[
                setup_layer(".git/orca-term.yaml", Some("")),
                setup_layer("orca-term.yaml", Some("make")),
            ]),
            None
        );
        assert_eq!(
            resolve_setup(&[
                setup_layer(".git/orca-term.yaml", None),
                setup_layer("orca-term.yaml", Some("make")),
            ])
            .map(|s| s.value),
            Some("make".to_owned())
        );
    }

    #[test]
    fn only_a_script_from_a_committed_file_needs_approval() {
        let committed = [setup_layer("orca-term.yaml", Some("make"))];
        assert_eq!(
            committed_setup_file(&committed),
            Some(Path::new("/repo/orca-term.yaml"))
        );
        let overridden = [
            setup_layer(".git/orca-term.yaml", Some("make local")),
            setup_layer("orca-term.yaml", Some("make")),
        ];
        assert_eq!(committed_setup_file(&overridden), None, "the override wins");
        let global = [
            setup_layer("orca-term.yaml", None),
            setup_layer("/g/config.yaml", Some("make")),
        ];
        assert_eq!(committed_setup_file(&global), None, "the global file wins");
        let turned_off = [
            setup_layer(".git/orca-term.yaml", Some("")),
            setup_layer("orca-term.yaml", Some("make")),
        ];
        assert_eq!(committed_setup_file(&turned_off), None, "no script at all");
        assert_eq!(
            committed_setup_file(&[setup_layer("orca-term.yaml", Some(" \n"))]),
            None
        );
        assert_eq!(committed_setup_file(&[]), None);
    }

    #[test]
    fn the_sources_say_which_one_is_committed() {
        let sources = sources(
            &primary(),
            Path::new("/w/repo/fix"),
            Path::new("/repo/.git"),
            &env(),
        );
        let committed: Vec<bool> = sources.iter().map(|s| s.committed).collect();
        assert_eq!(committed, [false, true, false]);
    }

    #[test]
    fn the_writable_paths_are_parsed_from_the_sandbox_mapping() {
        let raw = parse("sandbox:\n  writable:\n    - ~/.cache/pnpm\n    - ../shared\n").unwrap();
        assert_eq!(
            raw.sandbox.unwrap().writable.unwrap(),
            ["~/.cache/pnpm", "../shared"]
        );
        assert!(parse("sandbox:\n").unwrap().sandbox.is_none());
        let err = parse("sandbox:\n  enabled: false\n").unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown field `enabled`"),
            "{err:#}"
        );
        assert!(parse("sandbox:\n  writable: /a\n").is_err());
    }

    #[test]
    fn the_highest_layer_setting_writable_paths_replaces_the_lower_ones() {
        let writable = resolve_writable(
            &[
                writable_layer(".git/orca-term.yaml", "/repo", &["~/.cache/pnpm"]),
                writable_layer("orca-term.yaml", "/w/repo/fix", &["../shared", "/opt/x"]),
            ],
            &env(),
        )
        .unwrap();
        assert_eq!(writable.value, [PathBuf::from("/home/me/.cache/pnpm")]);
        assert_eq!(
            writable.origin(),
            "from .git/orca-term.yaml, overriding orca-term.yaml"
        );
        let writable = resolve_writable(
            &[writable_layer(
                "orca-term.yaml",
                "/w/repo/fix",
                &["../shared", "/opt/x"],
            )],
            &env(),
        )
        .unwrap();
        assert_eq!(
            writable.value,
            [PathBuf::from("/w/repo/shared"), PathBuf::from("/opt/x")]
        );
        assert_eq!(writable.origin(), "from orca-term.yaml");
        let none = resolve_writable(&[layer("orca-term.yaml", "/r", None)], &env()).unwrap();
        assert!(none.value.is_empty());
        assert_eq!(none.origin(), "built-in default");
        let err = resolve_writable(&[writable_layer("orca-term.yaml", "/r", &[" "])], &env())
            .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("orca-term.yaml: invalid `sandbox.writable`"),
            "{err:#}"
        );
    }

    #[test]
    fn the_global_file_follows_xdg_and_ignores_a_relative_xdg_dir() {
        let mut env = env();
        assert_eq!(
            env.global_file(),
            Some(PathBuf::from("/home/me/.config/orca-term/config.yaml"))
        );
        env.xdg_config_home = Some(PathBuf::from("/xdg"));
        assert_eq!(
            env.global_file(),
            Some(PathBuf::from("/xdg/orca-term/config.yaml"))
        );
        env.xdg_config_home = Some(PathBuf::from("rel"));
        assert_eq!(
            env.global_file(),
            Some(PathBuf::from("/home/me/.config/orca-term/config.yaml"))
        );
    }
}
