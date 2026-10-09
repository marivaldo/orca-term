//! Vocabulary lint: the glossary's *Avoid* synonyms, and `orca.yaml`, never appear in code or docs.
//!
//! Every term in an `_Avoid_:` line of `CONTEXT.md` is enforced unless the waiver file names it
//! with a reason. A waiver for a term the glossary no longer avoids is itself an error.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const GLOSSARY: &str = "CONTEXT.md";
const WAIVERS: &str = "docs/vocabulary-waivers.md";
/// Banned outright: the config file is `orca-term.yaml`.
const ALWAYS: &[&str] = &["orca.yaml"];
/// Paths never scanned: the glossary itself, decision records that discuss rejected terms, the
/// skill setup docs, the map's original input, the waiver file and this lint's own source.
const SKIP: &[&str] = &[
    GLOSSARY,
    WAIVERS,
    "docs/adr/",
    "docs/agents/",
    "docs/wayfinder-input.md",
    "xtask/src/vocab.rs",
    "Cargo.lock",
];

pub(crate) fn lint_repository(root: &Path) -> Result<Vec<String>> {
    let glossary = std::fs::read_to_string(root.join(GLOSSARY)).context("reading CONTEXT.md")?;
    let waivers = std::fs::read_to_string(root.join(WAIVERS)).unwrap_or_default();
    let avoided = avoided_terms(&glossary);
    let waived = parse_waivers(&waivers)?;
    let mut problems: Vec<String> = waived
        .keys()
        .filter(|t| !avoided.contains(t))
        .map(|t| format!("{WAIVERS}: `{t}` is waived but CONTEXT.md no longer avoids it"))
        .collect();
    let mut terms: Vec<String> = avoided
        .into_iter()
        .filter(|t| !waived.contains_key(t))
        .collect();
    terms.extend(ALWAYS.iter().map(|t| (*t).to_owned()));

    let listing = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z"])
        .output()?;
    if !listing.status.success() {
        bail!("git ls-files failed");
    }
    for path in listing.stdout.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path);
        if SKIP
            .iter()
            .any(|s| path == *s || (s.ends_with('/') && path.starts_with(s)))
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(root.join(path.as_ref())) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        problems.extend(
            find(&String::from_utf8_lossy(&bytes), &terms)
                .into_iter()
                .map(|(line, term)| {
                    format!("{path}:{line}: `{term}` is avoided by the glossary (CONTEXT.md)")
                }),
        );
    }
    Ok(problems)
}

/// Every term listed after `_Avoid_:` in the glossary, lowercased.
fn avoided_terms(glossary: &str) -> Vec<String> {
    let mut terms: Vec<String> = glossary
        .lines()
        .filter_map(|l| l.trim().strip_prefix("_Avoid_:"))
        .flat_map(|rest| rest.split(','))
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

/// Waivers are list items of the form ``- `term`: reason``. Each needs a reason.
fn parse_waivers(text: &str) -> Result<BTreeMap<String, String>> {
    let mut waived = BTreeMap::new();
    for line in text.lines().filter(|l| l.starts_with("- `")) {
        let rest = &line[3..];
        let Some((term, reason)) = rest.split_once("`:") else {
            bail!("{WAIVERS}: malformed waiver: {line}");
        };
        if reason.trim().is_empty() {
            bail!("{WAIVERS}: `{term}` is waived without a reason");
        }
        waived.insert(term.to_lowercase(), reason.trim().to_owned());
    }
    Ok(waived)
}

/// Whole-word, case-insensitive occurrences of `terms`, as (1-based line, term).
fn find(text: &str, terms: &[String]) -> Vec<(usize, String)> {
    let mut hits = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let lower = line.to_lowercase();
        for term in terms {
            if contains_word(&lower, term) {
                hits.push((n + 1, term.clone()));
            }
        }
    }
    hits
}

fn contains_word(haystack: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric();
    haystack.match_indices(word).any(|(i, _)| {
        let before = haystack[..i].chars().next_back();
        let after = haystack[i + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_avoid_line() {
        let g =
            "**Worktree**:\nText.\n_Avoid_: unit, Task\n\n**Fleet**:\n_Avoid_: swarm, main tree\n";
        assert_eq!(avoided_terms(g), vec!["main tree", "swarm", "task", "unit"]);
    }

    #[test]
    fn matches_whole_words_only() {
        assert!(contains_word("a swarm of agents", "swarm"));
        assert!(contains_word("worker_pool", "pool"));
        assert!(!contains_word("swarming", "swarm"));
        assert!(!contains_word("community", "unit"));
        assert!(contains_word("see orca.yaml", "orca.yaml"));
        assert!(!contains_word("see orca-term.yaml", "orca.yaml"));
    }

    #[test]
    fn waivers_need_a_reason() {
        assert!(parse_waivers("- `new`: Rust's constructor idiom").is_ok());
        assert!(parse_waivers("- `new`:  ").is_err());
    }

    #[test]
    fn reports_line_numbers() {
        let terms = vec!["swarm".to_owned()];
        assert_eq!(
            find("ok\nthe swarm\n", &terms),
            vec![(2, "swarm".to_owned())]
        );
    }
}
