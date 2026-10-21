//! What the recording tap wrote, made a recording: its local paths replaced by the placeholders the
//! replaying suites write back (`{{worktree}}`, `{{transcripts}}`, `{{tasks}}`, `{{denied}}`), and its file
//! named after the agent's version, so that a new version lands beside the old ones. Also the kind
//! of each hook or line, for `wait-for`.

use std::path::Path;

use serde_json::Value;

use super::scenario::Agent;

/// The file a recording of `recording` made with the agent's `version` is written to:
/// `<recording>-<version>.log`.
pub(crate) fn file_name(recording: &str, version: &str) -> String {
    format!("{recording}-{version}.log")
}

/// The kind of each line the tap wrote for `agent`: claude's hook event (the line's first word),
/// or the `type` of opencode's line (empty when the line is not JSON naming one).
pub(crate) fn kinds(agent: Agent, tapped: &str) -> Vec<String> {
    tapped
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| match agent {
            Agent::ClaudeCode => line.split(' ').next().unwrap_or_default().to_owned(),
            Agent::OpenCode => serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|value| value["type"].as_str().map(str::to_owned))
                .unwrap_or_default(),
        })
        .collect()
}

/// How many lines of `kinds` are of `kind`.
pub(crate) fn count(kinds: &[String], kind: &str) -> usize {
    kinds.iter().filter(|each| *each == kind).count()
}

/// The directory claude keeps its transcripts in, from the first `transcript_path` a hook of the
/// tapped text carries: the placeholder `{{transcripts}}` stands for it.
pub(crate) fn transcripts_dir(tapped: &str) -> Option<String> {
    tapped.lines().find_map(|line| {
        let (_, payload) = line.split_once(' ')?;
        let value: Value = serde_json::from_str(payload).ok()?;
        let path = Path::new(value["transcript_path"].as_str()?);
        Some(path.parent()?.to_str()?.to_owned())
    })
}

/// The temporary directory claude keeps a project's sessions in (`/private/tmp/claude-<uid>/<the
/// project's path, escaped>`, holding `<session>/tasks` and `<session>/scratchpad`), from the first
/// `scratchpad_dir` a hook of the tapped text carries (claude 2.1.306 and later): the placeholder
/// `{{tasks}}` stands for it, as in the recordings made by hand before.
pub(crate) fn tasks_dir(tapped: &str) -> Option<String> {
    tapped.lines().find_map(|line| {
        let (_, payload) = line.split_once(' ')?;
        let value: Value = serde_json::from_str(payload).ok()?;
        let scratchpad = Path::new(value["scratchpad_dir"].as_str()?);
        Some(scratchpad.parent()?.parent()?.to_str()?.to_owned())
    })
}

/// The pairs of a local path and the placeholder standing for it, longest path first so that a
/// path inside another is replaced first. Each path also stands for itself without `/private`, the
/// way macOS names the same temporary directory both ways.
pub(crate) fn placeholders(paths: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
    let mut pairs: Vec<(String, &'static str)> = Vec::new();
    for (path, placeholder) in paths {
        if path.is_empty() {
            continue;
        }
        pairs.push(((*path).to_owned(), placeholder));
        if let Some(short) = path.strip_prefix("/private")
            && short.starts_with('/')
        {
            pairs.push((short.to_owned(), placeholder));
        }
    }
    pairs.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    pairs.dedup();
    pairs
}

/// `tapped` with every local path of `placeholders` replaced by its placeholder, every line kept,
/// ending with a newline.
pub(crate) fn normalise(tapped: &str, placeholders: &[(String, &'static str)]) -> String {
    let mut text = tapped.to_owned();
    for (path, placeholder) in placeholders {
        text = text.replace(path.as_str(), placeholder);
    }
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// `text` with `{{worktree}}` and `{{denied}}` made the scenario's own paths: what a scenario
/// types, and the files it checks are missing.
pub(crate) fn expand(text: &str, worktree: &str, denied: &str) -> String {
    text.replace("{{worktree}}", worktree)
        .replace("{{denied}}", denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKTREE: &str = "/private/var/folders/x/orca-term-live-1/worktrees/live";
    const DENIED: &str =
        "/private/var/folders/x/orca-term-live-1/repo/.git/worktrees/live/orca-term";
    const TRANSCRIPTS: &str =
        "/Users/me/.claude/projects/-private-var-folders-x-orca-term-live-1-worktrees-live";

    fn tapped() -> String {
        format!(
            concat!(
                "SessionStart {{\"session_id\":\"s1\",\"transcript_path\":\"{t}/s1.jsonl\",\"cwd\":\"{w}\"}}\n",
                "PreToolUse {{\"session_id\":\"s1\",\"cwd\":\"{w}\",\"tool_input\":{{\"command\":\"touch {d}/x\"}}}}\n",
                "Stop {{\"session_id\":\"s1\",\"cwd\":\"/var/folders/x/orca-term-live-1/worktrees/live\"}}"
            ),
            t = TRANSCRIPTS,
            w = WORKTREE,
            d = DENIED
        )
    }

    #[test]
    fn a_recording_is_named_after_the_agents_version() {
        assert_eq!(file_name("p-hello", "2.1.306"), "p-hello-2.1.306.log");
        assert_eq!(
            file_name("opencode-esc", "1.18.0"),
            "opencode-esc-1.18.0.log"
        );
    }

    #[test]
    fn the_local_paths_become_the_placeholders_the_suites_write_back() {
        let tapped = tapped();
        let transcripts = transcripts_dir(&tapped).unwrap();
        assert_eq!(transcripts, TRANSCRIPTS);
        let pairs = placeholders(&[
            (WORKTREE, "{{worktree}}"),
            (DENIED, "{{denied}}"),
            (&transcripts, "{{transcripts}}"),
        ]);
        assert_eq!(
            normalise(&tapped, &pairs),
            concat!(
                "SessionStart {\"session_id\":\"s1\",\"transcript_path\":\"{{transcripts}}/s1.jsonl\",\"cwd\":\"{{worktree}}\"}\n",
                "PreToolUse {\"session_id\":\"s1\",\"cwd\":\"{{worktree}}\",\"tool_input\":{\"command\":\"touch {{denied}}/x\"}}\n",
                "Stop {\"session_id\":\"s1\",\"cwd\":\"{{worktree}}\"}\n"
            ),
            "the path without /private too, and a newline at the end"
        );
    }

    #[test]
    fn a_path_inside_another_is_replaced_first() {
        let pairs = placeholders(&[
            ("/t/repo", "{{a}}"),
            ("/t/repo/.git/x", "{{b}}"),
            ("", "{{c}}"),
        ]);
        assert_eq!(
            pairs,
            [
                ("/t/repo/.git/x".to_owned(), "{{b}}"),
                ("/t/repo".to_owned(), "{{a}}")
            ]
        );
        assert_eq!(
            normalise("/t/repo/.git/x/y /t/repo/z\n", &pairs),
            "{{b}}/y {{a}}/z\n"
        );
    }

    #[test]
    fn each_line_has_its_kind_for_wait_for() {
        let claude = kinds(Agent::ClaudeCode, &tapped());
        assert_eq!(claude, ["SessionStart", "PreToolUse", "Stop"]);
        assert_eq!(count(&claude, "Stop"), 1);
        let opencode = kinds(
            Agent::OpenCode,
            "{\"type\":\"start\"}\n{\"type\":\"prompt\",\"text\":\"x\"}\nnot json\n{\"type\":\"idle\"}\n",
        );
        assert_eq!(opencode, ["start", "prompt", "", "idle"]);
        assert_eq!(count(&opencode, "prompt"), 1);
        assert_eq!(
            transcripts_dir("{\"type\":\"start\"}\n"),
            None,
            "opencode has none"
        );
    }

    #[test]
    fn claudes_temporary_project_directory_becomes_the_tasks_placeholder() {
        let project =
            "/private/tmp/claude-501/-private-var-folders-x-orca-term-live-1-worktrees-live";
        let tapped = format!(
            concat!(
                "SessionStart {{\"session_id\":\"s1\",\"scratchpad_dir\":\"{p}/s1/scratchpad\"}}\n",
                "PostToolUse {{\"tool_response\":\"Output is being written to: {p}/s1/tasks/b1.output\"}}\n"
            ),
            p = project
        );
        let tasks = tasks_dir(&tapped).unwrap();
        assert_eq!(tasks, project);
        assert_eq!(
            normalise(&tapped, &placeholders(&[(&tasks, "{{tasks}}")])),
            concat!(
                "SessionStart {\"session_id\":\"s1\",\"scratchpad_dir\":\"{{tasks}}/s1/scratchpad\"}\n",
                "PostToolUse {\"tool_response\":\"Output is being written to: {{tasks}}/s1/tasks/b1.output\"}\n"
            )
        );
        assert_eq!(
            tasks_dir("{\"type\":\"start\"}\n"),
            None,
            "opencode has none"
        );
    }

    #[test]
    fn typed_text_gets_the_scenarios_paths() {
        assert_eq!(
            expand("touch {{denied}}/x in {{worktree}}", "/w", "/d"),
            "touch /d/x in /w"
        );
    }
}
