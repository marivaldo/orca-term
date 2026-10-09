//! Ops: `worktree new`, which creates git's worktree at `<base>/<repo>/<name>` on a new branch
//! from the default branch, copies the copy list in and writes the worktree's state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::config::{self, Config, Env, Layer};
use crate::domain::fleet::PrimaryCheckout;
use crate::domain::include;
use crate::domain::state::{self, WorktreeFile};
use crate::domain::worktree::{self, Created, WorktreeName};

/// Creates the worktree `name` in the repository containing the current directory.
pub(crate) fn run(name: &WorktreeName) -> Result<Created> {
    let cwd = env::current_dir()?;
    let env = env::config_env();
    let listed = git::primary_checkout(&cwd)?;
    let primary = PrimaryCheckout {
        path: fs::canonicalize(&listed.path)?,
        branch: listed.branch,
    };
    // The name already follows git's rules for a branch name; git still has the last word, in
    // case a newer git adds a rule.
    if !git::is_valid_branch_name(&primary.path, name.as_str())? {
        bail!("invalid worktree name `{name}`: it is not a valid branch name");
    }

    let config = load_config(&primary, &git::common_dir(&primary)?, &env)?;
    let path = worktree::path_for(&config.base.value, &primary, name)?;
    if fs::physical(&path).starts_with(&primary.path) {
        bail!(
            "the worktree would be at {}, inside the primary checkout; set `base` to a directory \
             outside it (base: {})",
            path.display(),
            config.base
        );
    }
    if fs::exists(&path) {
        bail!("{} already exists", path.display());
    }
    let start = git::start_point(&primary)?;
    let branch = name.branch();

    fs::create_dir_all(path.parent().unwrap_or(&path))?;
    git::worktree_add(&primary, &branch, &path, &start)?;

    // From here on the worktree exists, and nothing is undone: a failure leaves it in place.
    let copied = finish(&primary, &path).with_context(|| {
        format!(
            "the worktree was created at {} and left in place, but setting it up failed",
            path.display()
        )
    })?;
    Ok(Created {
        name: name.clone(),
        path,
        branch,
        start,
        copied,
        base: config.base,
    })
}

/// Reads every config file that exists, highest precedence first, and resolves them.
fn load_config(primary: &PrimaryCheckout, common_dir: &Path, env: &Env) -> Result<Config> {
    let mut layers = Vec::new();
    for source in config::sources(primary, common_dir, env) {
        if let Some(text) = fs::read_text_optional(&source.path)? {
            layers.push(Layer::parse(source, &text)?);
        }
    }
    config::resolve(&layers, env)
}

/// Copies the copy list and writes the worktree's state. Returns how many files were copied.
fn finish(primary: &PrimaryCheckout, worktree: &Path) -> Result<usize> {
    let copied = copy_include_list(primary, worktree, &include_list(primary)?)?;
    write_fresh_state(worktree)?;
    Ok(copied)
}

/// The repository-relative paths to copy from `primary`. Empty when there is no
/// `.worktreeinclude`.
fn include_list(primary: &PrimaryCheckout) -> Result<Vec<PathBuf>> {
    let patterns = primary.path.join(include::FILE_NAME);
    if !fs::is_file(&patterns) {
        return Ok(Vec::new());
    }
    let matching = git::untracked_matching(primary, &patterns)?;
    let ignored = git::untracked_ignored(primary)?;
    Ok(include::copyable(&matching, &ignored))
}

/// Copies each of `paths` from `primary` into `worktree` as plain bytes, returning how many files
/// were copied. Symlinks and anything else that is not a regular file are skipped, and nothing is
/// written outside the worktree.
fn copy_include_list(
    primary: &PrimaryCheckout,
    worktree: &Path,
    paths: &[PathBuf],
) -> Result<usize> {
    let worktree_real = fs::canonicalize(worktree)?;
    let mut copied = 0;
    for rel in paths {
        if !include::stays_inside(rel) {
            bail!(
                "refusing to copy {}: it escapes the worktree",
                rel.display()
            );
        }
        let from = primary.path.join(rel);
        let Some(mode) = fs::regular_file_mode(&from)? else {
            continue;
        };
        let to = worktree.join(rel);
        let parent = to.parent().unwrap_or(worktree);
        fs::create_dir_all(parent)?;
        if !fs::canonicalize(parent)?.starts_with(&worktree_real) || fs::is_symlink(&to) {
            bail!(
                "refusing to copy {}: it would land outside the worktree",
                rel.display()
            );
        }
        fs::byte_copy(&from, &to, mode)
            .with_context(|| format!("could not copy {} into the worktree", rel.display()))?;
        copied += 1;
    }
    Ok(copied)
}

/// Writes the state of a worktree just created, refusing to overwrite a newer schema.
fn write_fresh_state(worktree: &Path) -> Result<()> {
    let admin = git::admin_dir(worktree)?;
    let path = admin.state_file();
    state::check_overwrite(&path, fs::read_optional(&path)?.as_deref())?;
    fs::create_dir_all(admin.path())?;
    fs::write_atomically(&path, &WorktreeFile::fresh().to_bytes()?)
}
