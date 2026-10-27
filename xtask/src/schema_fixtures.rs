//! Schema fixtures lint (#118): every schema a state file is written with has frozen samples.
//!
//! Each state file's newest schema is a constant named `*_SCHEMA` under `src/`
//! (`pub(crate) const STATUS_SCHEMA: u64 = 1;`). For each value found, `tests/fixtures/schema-<n>/`
//! must exist: a schema rises only together with samples of the new schema, and a migration test
//! from the old one (ARCHITECTURE.md, "State and schema"). Finding no constant at all is a problem
//! too, so the lint cannot pass by no longer recognising them.

use std::path::Path;

use anyhow::{Context, Result};

use crate::launch_guard::collect_files;

/// Where the samples of each schema live, relative to the repository root.
const FIXTURES: &str = "tests/fixtures";

/// One `*_SCHEMA` constant: where it is, its name and its value.
#[derive(Debug, PartialEq, Eq)]
struct SchemaConstant {
    place: String,
    name: String,
    value: u64,
}

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();
    collect_files(&root.join("src"), "rs", &mut files)?;
    files.sort();
    let mut constants = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let rel = file.strip_prefix(root).unwrap_or(&file);
        constants.extend(schema_constants(&rel.to_string_lossy(), &text));
    }
    Ok(check(&constants, |schema| {
        root.join(fixtures_dir(schema)).is_dir()
    }))
}

/// The directory of the samples of `schema`, relative to the repository root.
fn fixtures_dir(schema: u64) -> String {
    format!("{FIXTURES}/schema-{schema}/")
}

/// The problems with `constants`, given which schemas have samples (`has_fixtures`).
fn check(constants: &[SchemaConstant], has_fixtures: impl Fn(u64) -> bool) -> Vec<String> {
    if constants.is_empty() {
        return vec![
            "found no `const *_SCHEMA: u64` under src/: the schema fixtures lint no longer \
             recognises the state schemas"
                .to_owned(),
        ];
    }
    constants
        .iter()
        .filter(|constant| !has_fixtures(constant.value))
        .map(|constant| {
            format!(
                "{}: {} is {} but {} is missing: a schema rises only with samples of the new \
                 schema and a migration test from the old one (ARCHITECTURE.md, \"State and \
                 schema\")",
                constant.place,
                constant.name,
                constant.value,
                fixtures_dir(constant.value)
            )
        })
        .collect()
}

/// The `*_SCHEMA` constants declared in the file at `path` holding `text`.
fn schema_constants(path: &str, text: &str) -> Vec<SchemaConstant> {
    text.lines()
        .enumerate()
        .filter_map(|(n, line)| {
            let (name, value) = schema_constant(line)?;
            Some(SchemaConstant {
                place: format!("{path}:{}", n + 1),
                name: name.to_owned(),
                value,
            })
        })
        .collect()
}

/// The name and value of the `*_SCHEMA` constant `line` declares, if it declares one:
/// `[pub(crate) ]const <NAME>_SCHEMA: u64 = <n>;`.
fn schema_constant(line: &str) -> Option<(&str, u64)> {
    let line = line.trim_start();
    let line = line.strip_prefix("pub(crate) ").unwrap_or(line);
    let rest = line.strip_prefix("const ")?;
    let (name, rest) = rest.split_once(':')?;
    if !name.ends_with("_SCHEMA") {
        return None;
    }
    let value = rest.trim().strip_prefix("u64")?.trim_start();
    let value = value.strip_prefix('=')?.trim().strip_suffix(';')?;
    Some((name, value.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(name: &str, value: u64) -> SchemaConstant {
        SchemaConstant {
            place: "src/domain/turn.rs:32".to_owned(),
            name: name.to_owned(),
            value,
        }
    }

    #[test]
    fn reads_each_schema_constant_with_its_line() {
        let text = "/// The newest schema.\npub(crate) const STATUS_SCHEMA: u64 = 1;\n\
                    const TURNS_SCHEMA: u64 = 2;\nconst WAIT: u64 = 3;\n\
                    // const OLD_SCHEMA: u64 = 9;\nlet schema = STATUS_SCHEMA;\n";
        assert_eq!(
            schema_constants("src/domain/turn.rs", text),
            [
                SchemaConstant {
                    place: "src/domain/turn.rs:2".to_owned(),
                    name: "STATUS_SCHEMA".to_owned(),
                    value: 1,
                },
                SchemaConstant {
                    place: "src/domain/turn.rs:3".to_owned(),
                    name: "TURNS_SCHEMA".to_owned(),
                    value: 2,
                },
            ]
        );
    }

    #[test]
    fn a_schema_with_its_samples_passes() {
        let constants = [constant("STATUS_SCHEMA", 1), constant("PR_SCHEMA", 2)];
        assert!(check(&constants, |schema| schema <= 2).is_empty());
    }

    #[test]
    fn a_schema_raised_without_samples_is_refused_saying_why() {
        let constants = [constant("STATUS_SCHEMA", 2)];
        assert_eq!(
            check(&constants, |schema| schema == 1),
            [
                "src/domain/turn.rs:32: STATUS_SCHEMA is 2 but tests/fixtures/schema-2/ is \
                 missing: a schema rises only with samples of the new schema and a migration \
                 test from the old one (ARCHITECTURE.md, \"State and schema\")"
            ]
        );
    }

    #[test]
    fn finding_no_constant_is_a_problem() {
        assert_eq!(check(&[], |_| true).len(), 1);
    }

    #[test]
    fn the_repository_has_samples_for_every_schema_it_writes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(lint_repository(&root).unwrap(), Vec::<String>::new());
    }
}
