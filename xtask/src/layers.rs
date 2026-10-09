//! Layers lint: the core's modules sit in five layers (ADR 0010, `ARCHITECTURE.md`), and code may
//! use only its own layer and the layers below it.
//!
//! For every `.rs` file under `src/` it checks that:
//! - the file's top-level module is in [`MODULES`], the table mapping modules to layers;
//! - the first line is a `//!` naming the file's layer, such as `//! Domain: ...`;
//! - every `crate::<module>` path, in a `use` item or inline, names the same layer or a lower one;
//! - the opt-out from Clippy's `disallowed-methods` and `disallowed-types` appears only in adapters.
//!
//! The check is textual, as rust-analyzer's own tidy checks are: comments are skipped, and
//! `super::` paths are not followed, so code reaches other modules through `crate::`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The layers, lowest first: each may use its own layer and the ones before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Domain,
    Adapters,
    Ops,
    Edge,
    Main,
}

impl Layer {
    fn name(self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::Adapters => "adapters",
            Self::Ops => "ops",
            Self::Edge => "edge",
            Self::Main => "main",
        }
    }

    /// How a file of this layer must start.
    fn first_line(self) -> &'static str {
        match self {
            Self::Domain => "//! Domain: ",
            Self::Adapters => "//! Adapter: ",
            Self::Ops => "//! Ops: ",
            Self::Edge => "//! Edge: ",
            Self::Main => "//! Main: ",
        }
    }
}

/// Every top-level module of `src/` and its layer. A file belongs to the module its path starts
/// with: `src/ops.rs` and `src/ops/worktree_new.rs` are both `ops`. A new module is added here on
/// purpose, or the lint fails.
const MODULES: &[(&str, Layer)] = &[
    ("main", Layer::Main),
    ("lib", Layer::Main),
    ("cli", Layer::Edge),
    ("output", Layer::Edge),
    ("ops", Layer::Ops),
    ("adapters", Layer::Adapters),
    ("domain", Layer::Domain),
];

/// The lints only adapter modules may opt out of.
const ADAPTER_ONLY_LINTS: &[&str] = &["clippy::disallowed_methods", "clippy::disallowed_types"];

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_rust_files(&root.join("src"), &mut files)?;
    files.sort();
    let mut problems = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let rel = file.strip_prefix(root).unwrap_or(&file);
        problems.extend(check_file(&rel.to_string_lossy(), &text));
    }
    Ok(problems)
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// The findings for the file at `path` (such as `src/ops.rs`) holding `text`, each as
/// `path:line: message`.
fn check_file(path: &str, text: &str) -> Vec<String> {
    let module = module_of(path);
    let Some(layer) = layer_of(module) else {
        return vec![format!(
            "{path}:1: module `{module}` is not in the layer table (MODULES in xtask/src/layers.rs)"
        )];
    };
    let mut problems = Vec::new();
    if !text.starts_with(layer.first_line()) {
        problems.push(format!(
            "{path}:1: the file must start with `{}...`, naming its layer ({})",
            layer.first_line(),
            layer.name()
        ));
    }
    if layer != Layer::Adapters {
        for (n, line) in text.lines().enumerate() {
            for lint in ADAPTER_ONLY_LINTS
                .iter()
                .filter(|lint| line.contains(*lint))
            {
                problems.push(format!(
                    "{path}:{}: only adapter modules may opt out of `{lint}`",
                    n + 1
                ));
            }
        }
    }
    let code = without_comments(text);
    for (offset, used) in crate_references(&code) {
        let line = code[..offset].matches('\n').count() + 1;
        match layer_of(used) {
            None => problems.push(format!(
                "{path}:{line}: `crate::{used}` is not in the layer table (MODULES in xtask/src/layers.rs)"
            )),
            Some(target) if target > layer => problems.push(format!(
                "{path}:{line}: {} code uses `crate::{used}`, from the higher {} layer",
                layer.name(),
                target.name()
            )),
            Some(_) => {}
        }
    }
    problems
}

/// The top-level module a file under `src/` belongs to: `src/output/json.rs` is `output`.
fn module_of(path: &str) -> &str {
    let rel = path.strip_prefix("src/").unwrap_or(path);
    let first = rel.split('/').next().unwrap_or(rel);
    first.strip_suffix(".rs").unwrap_or(first)
}

fn layer_of(module: &str) -> Option<Layer> {
    MODULES
        .iter()
        .find(|(name, _)| *name == module)
        .map(|(_, layer)| *layer)
}

/// `text` with everything from `//` to the end of each line removed, keeping the line breaks.
fn without_comments(text: &str) -> String {
    text.lines()
        .map(|line| line.find("//").map_or(line, |at| &line[..at]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every top-level module a `crate::` path in `code` names, with the byte offset of its name.
/// `crate::{a::b, c}` names both `a` and `c`.
fn crate_references(code: &str) -> Vec<(usize, &str)> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices("crate::") {
        let before = code[..at].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            continue;
        }
        let start = at + "crate::".len();
        if code[start..].starts_with('{') {
            found.extend(group_items(code, start + 1));
        } else {
            found.extend(identifier_at(code, start));
        }
    }
    found
}

/// The leading identifier of each item in the `{...}` group whose contents start at `start`.
fn group_items(code: &str, start: usize) -> Vec<(usize, &str)> {
    let mut items = Vec::new();
    let mut depth = 0;
    let mut item_start = start;
    for (i, c) in code[start..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => {
                items.extend(identifier_at(code, item_start));
                break;
            }
            '}' => depth -= 1,
            ',' if depth == 0 => {
                items.extend(identifier_at(code, item_start));
                item_start = start + i + 1;
            }
            _ => {}
        }
    }
    items.retain(|(_, name)| *name != "self");
    items
}

/// The identifier at `start` in `code`, skipping leading whitespace, with its offset.
fn identifier_at(code: &str, start: usize) -> Option<(usize, &str)> {
    let rest = &code[start..];
    let skipped = rest.len() - rest.trim_start().len();
    let rest = &rest[skipped..];
    let len = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
        .unwrap_or(rest.len());
    (len > 0).then(|| (start + skipped, &rest[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_may_use_itself_and_the_layers_below() {
        let text = "//! Ops: x.\n\nuse crate::adapters::{fs, git};\nuse crate::domain::config;\n\
                    use crate::ops::read_fleet;\nfn f() { crate::domain::state::read(); }\n";
        assert_eq!(
            check_file("src/ops/worktree_new.rs", text),
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_upward_import_is_rejected_with_its_line() {
        let text = "//! Domain: x.\n\nuse crate::adapters::git;\n";
        assert_eq!(
            check_file("src/domain/fleet.rs", text),
            [
                "src/domain/fleet.rs:3: domain code uses `crate::adapters`, from the higher adapters layer"
            ]
        );
    }

    #[test]
    fn an_upward_path_inside_a_group_or_inline_is_rejected() {
        let text = "//! Adapter: x.\nuse crate::{\n    domain::fleet,\n    ops::read_fleet,\n};\n\
                    fn f() { crate::output::error(); }\n";
        assert_eq!(
            check_file("src/adapters/git.rs", text),
            [
                "src/adapters/git.rs:4: adapters code uses `crate::ops`, from the higher ops layer",
                "src/adapters/git.rs:6: adapters code uses `crate::output`, from the higher edge layer",
            ]
        );
    }

    #[test]
    fn comments_and_other_crates_are_not_references() {
        let text = "//! Domain: see `crate::ops`.\n// crate::cli\nuse other_crate::ops;\n";
        assert!(check_file("src/domain/state.rs", text).is_empty());
    }

    #[test]
    fn a_file_without_its_layer_line_is_rejected() {
        assert_eq!(
            check_file("src/domain/state.rs", "use std::path::Path;\n"),
            [
                "src/domain/state.rs:1: the file must start with `//! Domain: ...`, naming its layer (domain)"
            ]
        );
        assert_eq!(
            check_file("src/domain/state.rs", "//! Adapter: wrong layer.\n").len(),
            1
        );
    }

    #[test]
    fn the_opt_out_is_for_adapters_only() {
        let opt_out = "#![expect(clippy::disallowed_methods, reason = \"x\")]\n";
        assert!(
            check_file("src/adapters/fs.rs", &format!("//! Adapter: x.\n{opt_out}")).is_empty()
        );
        assert_eq!(
            check_file("src/domain/state.rs", &format!("//! Domain: x.\n{opt_out}")),
            [
                "src/domain/state.rs:2: only adapter modules may opt out of `clippy::disallowed_methods`"
            ]
        );
    }

    #[test]
    fn every_module_is_in_the_table() {
        assert_eq!(
            check_file("src/extra.rs", "//! Domain: x.\n"),
            [
                "src/extra.rs:1: module `extra` is not in the layer table (MODULES in xtask/src/layers.rs)"
            ]
        );
        assert_eq!(module_of("src/output/json.rs"), "output");
        assert_eq!(module_of("src/main.rs"), "main");
    }
}
