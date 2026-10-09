//! Domain: the configuration, YAML files layered key by key.
//!
//! Precedence, highest first: the uncommitted override `orca-term.yaml` in the git common dir, the
//! committed `orca-term.yaml` at the primary checkout root, the global
//! `$XDG_CONFIG_HOME/orca-term/config.yaml` (or `~/.config/orca-term/config.yaml`), then the
//! built-in default. Every resolved key remembers which source won and which it overrode, so the
//! precedence can be printed.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_norway::Value;

use crate::domain::fleet::PrimaryCheckout;

/// The config file's name, both committed and in the git common dir.
pub(crate) const FILE_NAME: &str = "orca-term.yaml";

/// Every key this version understands.
const KEYS: &[&str] = &["base"];

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

impl fmt::Display for Setting<PathBuf> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value.display())?;
        match &self.source {
            None => write!(f, " (built-in default)"),
            Some(source) if self.overridden.is_empty() => write!(f, " (from {source})"),
            Some(source) => write!(
                f,
                " (from {source}, overriding {})",
                self.overridden.join(", ")
            ),
        }
    }
}

/// The resolved configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Config {
    /// The directory worktrees live under, as `<base>/<repo>/<name>`.
    pub(crate) base: Setting<PathBuf>,
}

/// Where a config file may be, how the precedence line names it and what relative paths in it
/// resolve against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    label: String,
    anchor: PathBuf,
}

/// One config file that exists, parsed.
#[derive(Debug)]
pub(crate) struct Layer {
    /// How the precedence line names it.
    label: String,
    /// What relative paths in it resolve against.
    anchor: PathBuf,
    raw: Raw,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    base: Option<String>,
}

/// Every config file of the repository whose primary checkout is `primary` and whose git common
/// dir is `common_dir`, highest precedence first. Some of them may not exist.
pub(crate) fn sources(primary: &PrimaryCheckout, common_dir: &Path, env: &Env) -> Vec<Source> {
    let root = &primary.path;
    let local = common_dir.join(FILE_NAME);
    let mut sources = vec![
        Source {
            label: label_within(&local, root),
            path: local,
            anchor: root.clone(),
        },
        Source {
            path: root.join(FILE_NAME),
            label: FILE_NAME.to_owned(),
            anchor: root.clone(),
        },
    ];
    if let Some(global) = env.global_file() {
        sources.push(Source {
            label: global.display().to_string(),
            anchor: global.parent().map(Path::to_owned).unwrap_or_default(),
            path: global,
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
            label: source.label,
            anchor: source.anchor,
            raw,
        })
    }
}

/// Resolves `layers`, highest precedence first, over the built-in defaults.
pub(crate) fn resolve(layers: &[Layer], env: &Env) -> Result<Config> {
    let home = env.home.as_deref();
    let mut setters = layers
        .iter()
        .filter_map(|l| l.raw.base.as_deref().map(|v| (l, v)));
    let base = match setters.next() {
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
    };
    Ok(Config { base })
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
            label: label.to_owned(),
            anchor: PathBuf::from(anchor),
            raw: Raw {
                base: base.map(str::to_owned),
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
        let config = resolve(&[], &env()).unwrap();
        assert_eq!(
            config.base.value,
            PathBuf::from("/home/me/orca-term/worktrees")
        );
        assert_eq!(config.base.source, None);
        assert_eq!(
            config.base.to_string(),
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
        let base = resolve(&layers, &env()).unwrap().base;
        assert_eq!(
            base.to_string(),
            "/l (from .git/orca-term.yaml, overriding /g/config.yaml)"
        );
    }

    #[test]
    fn relative_paths_resolve_against_the_layer_anchor() {
        let base = resolve(
            &[layer("orca-term.yaml", "/w/repo", Some("../worktrees"))],
            &env(),
        )
        .unwrap()
        .base;
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
        let sources = sources(&primary(), Path::new("/repo/.git"), &env());
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
        let source = sources(&primary(), Path::new("/repo/.git"), &env()).remove(1);
        let err = Layer::parse(source, "bogus: 1\n").unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            "invalid config file /repo/orca-term.yaml: unknown key `bogus` (known keys: base)"
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
