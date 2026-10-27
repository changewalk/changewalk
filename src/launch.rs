//! Checking the merge request's branch out and opening it in Zed.

use crate::gitlab::{MergeRequest, git};
use anyhow::{Context as _, Result, bail};
use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Checks the branch out, hands the merge request to Zed as a file, and opens
/// the diff with a suggested review order. Returns a line saying what
/// happened, for the picker's status, since it runs while the picker owns
/// the terminal.
pub fn open_in_zed(merge_request: &MergeRequest, zed_app: Option<PathBuf>) -> Result<String> {
    let zed_app = zed_app.unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_default()
            .join("Applications/Zed Dev.app")
    });
    let warning = check_out(merge_request)?;
    let file = write_merge_request_file(merge_request)?;

    let mut query = vec![("repo", merge_request.repo.to_string_lossy().into_owned())];
    if merge_request.stacked {
        // Diff a stacked merge request against its own base, so only its
        // changes show.
        query.push(("base", format!("origin/{}", merge_request.target_branch)));
    }
    query.push(("mr", file.to_string_lossy().into_owned()));
    let url = format!(
        "zed://git/review?{}",
        query
            .iter()
            .map(|(key, value)| format!("{key}={}", urlencoding::encode(value)))
            .collect::<Vec<_>>()
            .join("&")
    );

    let mut open = Command::new("open");
    // A freshly launched app does not inherit the shell's environment, so
    // pass the model key through. A running app keeps the environment it
    // started with, and `open` warns if asked to change it, so skip it then.
    if !zed_is_running(&zed_app) {
        for key in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
            if let Ok(value) = std::env::var(key) {
                open.arg("--env").arg(format!("{key}={value}"));
            }
        }
    }
    let status = open
        .arg("-a")
        .arg(&zed_app)
        .arg(&url)
        .status()
        .context("running open")?;
    if !status.success() {
        bail!("could not open {}", zed_app.display());
    }
    let mut message = format!(
        "opened !{} ({}) in Zed",
        merge_request.iid, merge_request.source_branch
    );
    if let Some(warning) = warning {
        message.push_str(" · ");
        message.push_str(&warning);
    }
    Ok(message)
}

/// Fetches the branch, and its base for a stacked merge request, in one round
/// trip, then checks it out or fast-forwards it if it is already local. A
/// local branch that cannot fast-forward is kept as it is, with a warning.
fn check_out(merge_request: &MergeRequest) -> Result<Option<String>> {
    let repo = &merge_request.repo;
    if !git(repo, &["status", "--porcelain"])?.is_empty() {
        bail!(
            "the working tree at {} has uncommitted changes; commit or stash them first",
            repo.display()
        );
    }
    let source = merge_request.source_branch.as_str();
    let mut fetch = vec!["fetch", "origin", source];
    if merge_request.stacked {
        fetch.push(merge_request.target_branch.as_str());
    }
    git(repo, &fetch)?;

    let remote = format!("origin/{source}");
    let local_exists = git(
        repo,
        &["show-ref", "--verify", &format!("refs/heads/{source}")],
    )
    .is_ok();
    let current = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if !local_exists {
        git(repo, &["checkout", "--track", "-b", source, &remote])?;
    } else {
        if current != source {
            git(repo, &["checkout", source])?;
        }
        if git(repo, &["merge", "--ff-only", &remote]).is_err() {
            return Ok(Some(format!(
                "{source} is behind {remote} and did not fast-forward"
            )));
        }
    }
    Ok(None)
}

/// Writes the merge request as JSON for Zed to show above the diff, which
/// saves Zed two round trips to GitLab.
fn write_merge_request_file(merge_request: &MergeRequest) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("changewalk");
    std::fs::create_dir_all(&dir).context("creating the temporary directory")?;
    let file = dir.join(format!("{}.json", merge_request.iid));
    std::fs::write(&file, serde_json::to_string(merge_request)?)
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(file)
}

fn zed_is_running(zed_app: &Path) -> bool {
    let executable = zed_app.join("Contents/MacOS/zed");
    let executable = std::fs::canonicalize(&executable).unwrap_or(executable);
    Command::new("pgrep")
        .arg("-f")
        .arg(executable)
        .output()
        .is_ok_and(|output| output.status.success())
}
