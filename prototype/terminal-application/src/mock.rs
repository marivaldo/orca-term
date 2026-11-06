//! Placeholder content for the terminal panes: what the design's mocks show claude-code and
//! Neovim drawing (`claudeLines`, `failLines`, `nvim`). The snapshots draw it where a real
//! terminal would be; the live prototype only uses `fail_lines`, as the stopped agent's output.

use ratatui::style::Style;

use crate::paint::{Painter, Seg, bold, c, fg, italic, seg, wrap};
use crate::theme::{self, BG, BORDER, DONE, PANEL, RAISED, TEXT, TEXT_2, WAITING, WORKING};

pub type Lines = Vec<Vec<Seg>>;

fn plain() -> Style {
    fg(TEXT)
}

/// `segs` cut or padded to exactly `n` characters.
fn fit(segs: Vec<Seg>, n: usize) -> Vec<Seg> {
    let mut out = Vec::new();
    let mut used = 0;
    for (text, style) in segs {
        if used >= n {
            break;
        }
        let count = text.chars().count();
        let text = if count > n - used {
            crate::paint::trunc(&text, n - used)
        } else {
            text
        };
        used += text.chars().count();
        out.push((text, style));
    }
    if used < n {
        out.push(seg(" ".repeat(n - used), plain()));
    }
    out
}

/// claude-code asking which login flow should stay, wrapped to `w` columns.
pub fn claude_lines(w: usize) -> Lines {
    let mut l: Lines = Vec::new();
    let dim = fg(TEXT_2);
    let msg = |l: &mut Lines, bullet: &str, colour, text: &str, style: Style| {
        for (i, line) in wrap(text, w.saturating_sub(2)).into_iter().enumerate() {
            let lead = if i == 0 {
                format!("{bullet} ")
            } else {
                "  ".to_owned()
            };
            l.push(vec![seg(lead, bold(colour)), seg(line, style)]);
        }
    };
    l.push(vec![
        seg("claude", bold(TEXT)),
        seg("  ·  app/fix-login  ·  ~/code/app.worktrees/fix-login", dim),
    ]);
    l.push(vec![]);
    msg(
        &mut l,
        ">",
        TEXT_2,
        "The login page has two flows now, the old password form and the new magic link. \
         Clean it up so only one is left.",
        dim,
    );
    l.push(vec![]);
    msg(
        &mut l,
        "●",
        TEXT,
        "I'll check how both flows are wired before touching anything.",
        plain(),
    );
    l.push(vec![]);
    for (name, arg, result) in [
        ("Read", "src/auth/login.rs", "212 lines"),
        ("Read", "src/auth/magic_link.rs", "98 lines"),
        ("Search", "\"PasswordForm\"", "7 matches in 4 files"),
        (
            "Update",
            "src/auth/session.rs",
            "+6 −2  verify_session() shared by both flows",
        ),
    ] {
        tool(&mut l, name, arg, result);
    }
    l.push(vec![]);
    msg(
        &mut l,
        "●",
        TEXT,
        "Both flows share the session handler but split at verification. Dropping either one \
         touches routes, templates and tests, so I need a decision before going further.",
        plain(),
    );
    l.push(vec![]);

    let bw = w.min(78);
    let iw = bw - 4;
    let q = fg(WAITING);
    let title = " Which login flow should stay? ";
    l.push(vec![
        seg("╭─", q),
        seg(title, bold(WAITING)),
        seg(
            format!(
                "{}╮",
                "─".repeat(bw.saturating_sub(3 + title.chars().count()))
            ),
            q,
        ),
    ]);
    let row = |l: &mut Lines, segs: Vec<Seg>| {
        let mut line = vec![seg("│ ", q)];
        line.extend(fit(segs, iw));
        line.push(seg(" │", q));
        l.push(line);
    };
    row(&mut l, vec![]);
    let options = [
        (
            "Magic link only",
            "Remove the password form, its template and 4 tests.",
        ),
        ("Password only", "Remove magic_link.rs and the email job."),
        (
            "Keep both behind a flag",
            "Add LOGIN_FLOW to config, default to magic link.",
        ),
        ("Type something else…", ""),
    ];
    for (i, (title, detail)) in options.iter().enumerate() {
        let first = i == 0;
        row(
            &mut l,
            vec![
                seg(if first { "› " } else { "  " }, bold(WORKING)),
                seg(
                    format!("{}. {title}", i + 1),
                    if first { bold(WORKING) } else { plain() },
                ),
            ],
        );
        if !detail.is_empty() {
            for line in wrap(detail, iw - 5) {
                row(&mut l, vec![seg(format!("     {line}"), dim)]);
            }
        }
    }
    row(&mut l, vec![]);
    row(
        &mut l,
        vec![seg("↑↓ move · enter select · esc dismiss", dim)],
    );
    l.push(vec![seg(format!("╰{}╯", "─".repeat(bw - 2)), q)]);
    l.push(vec![]);
    l.push(vec![seg("  waiting for your answer · 2m", italic(TEXT_2))]);
    l
}

fn tool(l: &mut Lines, name: &str, arg: &str, result: &str) {
    l.push(vec![
        seg("● ", fg(DONE)),
        seg(name, bold(TEXT)),
        seg(format!(" {arg}"), plain()),
    ]);
    l.push(vec![seg("  └ ", fg(TEXT_2)), seg(result, fg(TEXT_2))]);
}

/// The output the agent of `retry-jobs` left before it stopped.
pub fn fail_lines(w: usize) -> Lines {
    let mut l: Lines = Vec::new();
    let dim = fg(TEXT_2);
    l.push(vec![
        seg("claude", bold(TEXT)),
        seg(
            "  ·  api/retry-jobs  ·  ~/code/api.worktrees/retry-jobs",
            dim,
        ),
    ]);
    l.push(vec![]);
    let ask = "Make the job runner retry failed jobs with exponential backoff, max 5 attempts.";
    for (i, line) in wrap(ask, w.saturating_sub(2)).into_iter().enumerate() {
        let lead = if i == 0 { "> " } else { "  " };
        l.push(vec![seg(lead, bold(TEXT_2)), seg(line, dim)]);
    }
    l.push(vec![]);
    for (name, arg, result) in [
        ("Read", "src/jobs/runner.rs", "184 lines"),
        ("Write", "src/jobs/backoff.rs", "+38"),
        ("Update", "src/jobs/runner.rs", "+22 −4"),
        ("Bash", "cargo test jobs::", "running…"),
    ] {
        tool(&mut l, name, arg, result);
    }
    l.push(vec![]);
    l.push(vec![
        seg("error: ", bold(theme::FAILED)),
        seg("stream closed: connection reset by peer", plain()),
    ]);
    l.push(vec![seg("claude exited with code 1", dim)]);
    l
}

/// `lines` from (`x`, `y`) in `h` rows of `w` columns: the last ones when they do not fit.
pub fn draw_lines(p: &mut Painter, x: u16, y: u16, w: u16, h: u16, lines: &Lines) {
    let start = lines.len().saturating_sub(usize::from(h));
    for (i, line) in lines.iter().skip(start).take(usize::from(h)).enumerate() {
        p.segs(x, y + i as u16, line, w);
    }
}

const CODE: [&str; 38] = [
    "use axum::{extract::State, Form};",
    "use axum::response::Redirect;",
    "use serde::Deserialize;",
    "",
    "use crate::auth::{magic_link, session::Session};",
    "use crate::{AppState, AuthError, User};",
    "",
    "#[derive(Deserialize)]",
    "pub struct LoginForm {",
    "    email: String,",
    "    password: Option<String>,",
    "}",
    "",
    "/// Single entry point for both login flows.",
    "pub async fn login(",
    "    State(app): State<AppState>,",
    "    Form(form): Form<LoginForm>,",
    ") -> Result<Redirect, AuthError> {",
    "    let user = app.users.by_email(&form.email).await?;",
    "",
    "    match form.password {",
    "        Some(pw) => verify_password(&user, &pw)?,",
    "        None => {",
    "            magic_link::send(&app, &user).await?;",
    "            return Ok(Redirect::to(\"/check-email\"));",
    "        }",
    "    }",
    "",
    "    let session = Session::create(&app.db, user.id).await?;",
    "    Ok(Redirect::to(\"/\").with_cookie(session.cookie()))",
    "}",
    "",
    "fn verify_password(user: &User, pw: &str) -> Result<(), AuthError> {",
    "    if !argon2::verify(&user.hash, pw)? {",
    "        return Err(AuthError::BadCredentials);",
    "    }",
    "    Ok(())",
    "}",
];

const KEYWORDS: [&str; 14] = [
    "use", "pub", "struct", "async", "fn", "let", "match", "return", "if", "await", "crate", "mut",
    "impl", "self",
];

/// The design's crude Rust highlighting: comments, strings, attributes, keywords, types, calls.
fn highlight(line: &str) -> Vec<Seg> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        let ch = chars[i];
        if rest.starts_with("//") {
            out.push(seg(rest, italic(TEXT_2)));
            break;
        }
        if ch == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '"' {
                j += 1;
            }
            let end = (j + 1).min(chars.len());
            out.push(seg(chars[i..end].iter().collect::<String>(), fg(DONE)));
            i = end;
            continue;
        }
        if rest.starts_with("#[") {
            let end = chars[i..]
                .iter()
                .position(|c| *c == ']')
                .map_or(chars.len(), |p| i + p + 1);
            out.push(seg(chars[i..end].iter().collect::<String>(), fg(TEXT_2)));
            i = end;
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            let next = chars.get(j).copied();
            let style = if KEYWORDS.contains(&word.as_str()) {
                fg(WORKING)
            } else if word.starts_with(|c: char| c.is_ascii_uppercase()) {
                fg(WAITING)
            } else if next == Some('(') || next == Some('!') {
                bold(TEXT)
            } else {
                plain()
            };
            out.push(seg(word, style));
            i = j;
            continue;
        }
        if ch.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            out.push(seg(chars[i..j].iter().collect::<String>(), fg(WAITING)));
            i = j;
            continue;
        }
        out.push(seg(ch.to_string(), plain()));
        i += 1;
    }
    out
}

/// Neovim on `src/auth/login.rs`, as the design draws it, in `h` rows of `w` columns.
pub fn nvim(p: &mut Painter, x: u16, y: u16, w: u16, h: u16, focused: bool) {
    p.fill(x, y, w, h, BG);
    let body = h.saturating_sub(2);
    let cursor_line = 21;
    for i in 0..body {
        let row = y + i;
        let Some(code) = CODE.get(usize::from(i)) else {
            p.text(x, row, "~", fg(BORDER));
            continue;
        };
        let current = usize::from(i) == cursor_line;
        if current {
            p.tint(x, row, w, 1, PANEL);
        }
        let sign = match i + 1 {
            14 | 21 | 29 => Some("~"),
            23..=26 => Some("+"),
            _ => None,
        };
        if let Some(sign) = sign {
            let colour = if sign == "+" { DONE } else { WORKING };
            p.text(x, row, sign, bold(colour));
        }
        let number = format!("{:>3}", i + 1);
        let style = if current { bold(TEXT) } else { fg(TEXT_2) };
        p.text(x + 1, row, &number, style);
        p.segs(x + 5, row, &highlight(code), w.saturating_sub(5));
    }
    if focused {
        let (cx, cy) = (x + 5 + 8, y + cursor_line as u16);
        if cx < x + w {
            let cell = &mut p.buf[(cx, cy)];
            cell.bg = c(TEXT);
            cell.fg = c(BG);
        }
    }
    let sy = y + h.saturating_sub(2);
    p.fill(x, sy, w, 1, RAISED);
    let sx = p.text(x, sy, " NORMAL ", bold(BG).bg(c(WORKING)));
    p.line(
        sx,
        sy,
        &[
            seg("  src/auth/login.rs", plain()),
            seg(" [+]", fg(WAITING)),
        ],
    );
    let right: Vec<Seg> = if w >= 80 {
        vec![
            seg("+5 ", fg(DONE)),
            seg("~3", fg(WORKING)),
            seg("    rust    utf-8    22:9    56% ", fg(TEXT_2)),
        ]
    } else {
        vec![seg("22:9  56% ", fg(TEXT_2))]
    };
    p.rsegs(x + w, sy, &right);
    if w >= 80 {
        p.text(
            x,
            sy + 1,
            "\"src/auth/login.rs\" 38L, 1291B written",
            fg(TEXT_2),
        );
    }
}
