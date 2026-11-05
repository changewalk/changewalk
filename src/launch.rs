//! Checking the merge request's branch out and opening it in Zed.

use crate::gitlab::{self, MergeRequest, Project, git};
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
    // The list may be hours old. A parent merge request merging retargets
    // this one and deletes the branch it was diffed against, so ask GitLab
    // for the merge request as it is now before checking anything out.
    let fresh = Project::discover(&merge_request.repo)
        .and_then(|project| gitlab::fetch_merge_request(&project, merge_request.iid));
    let (merge_request, mut warning) = match fresh {
        Ok(fresh) => {
            let mut fresh = fresh;
            fresh.depth = merge_request.depth;
            (fresh, None)
        }
        Err(error) => (
            merge_request.clone(),
            Some(format!("using the cached merge request: {error:#}")),
        ),
    };
    let mut merge_request = merge_request;
    // A branch already checked out in another worktree, as a tool-managed
    // workspace of the user's own work is, cannot be checked out again;
    // the review opens in that worktree instead.
    if let Some(worktree) = worktree_holding(&merge_request.repo, &merge_request.source_branch) {
        merge_request.repo = worktree;
    }
    let merge_request = &merge_request;
    if let Some(checkout_warning) = check_out(merge_request)? {
        warning.get_or_insert(checkout_warning);
    }
    let file = write_merge_request_file(merge_request)?;

    let mut query = vec![("repo", merge_request.repo.to_string_lossy().into_owned())];
    if let Some(base) = review_base(merge_request) {
        query.push(("base", base));
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

/// What Zed should diff the branch against, when not simply the default
/// branch's merge base.
///
/// A branch stacked on another keeps that one's commits. Once the parent is
/// squash-merged, those commits are in the target under new hashes, so a
/// merge-base diff shows them again. GitLab shows only the commits the
/// target does not have; `git cherry` finds the same set by patch, and the
/// base is the parent of the oldest such commit. A stacked merge request
/// whose target could not be fetched falls back to GitLab's base commit.
pub fn review_base(merge_request: &MergeRequest) -> Option<String> {
    let repo = &merge_request.repo;
    let target = format!("origin/{}", merge_request.target_branch);
    let source = format!("origin/{}", merge_request.source_branch);
    if let Ok(cherry) = git(repo, &["cherry", &target, &source]) {
        // Oldest first; `-` marks a commit the target already has.
        let marks: Vec<(bool, &str)> = cherry
            .lines()
            .filter_map(|line| {
                let (mark, sha) = line.split_once(' ')?;
                Some((mark == "+", sha.trim()))
            })
            .collect();
        let first_new = marks.iter().position(|(new, _)| *new);
        let already_merged_below = first_new.is_some_and(|index| index > 0);
        let contiguous = first_new.is_some_and(|index| marks[index..].iter().all(|(new, _)| *new));
        if already_merged_below && contiguous {
            let parent = format!("{}^", marks[first_new?].1);
            if let Ok(sha) = git(repo, &["rev-parse", &parent]) {
                return Some(sha);
            }
        }
        if !merge_request.stacked {
            return None;
        }
        return Some(target);
    }
    if !merge_request.stacked {
        return None;
    }
    Some(
        merge_request
            .diff_refs
            .as_ref()
            .map(|refs| refs.base_sha.clone())
            .unwrap_or(target),
    )
}

/// Lines added and removed in the diff Zed will show, counted from the
/// clone with the same base as the launch uses, so the list agrees with
/// the review. None when the branches are not fetched yet.
pub fn local_diff_size(merge_request: &MergeRequest) -> Option<(u64, u64)> {
    let repo = &merge_request.repo;
    let source = format!("origin/{}", merge_request.source_branch);
    let range = match review_base(merge_request) {
        Some(base) => format!("{base}..{source}"),
        None => format!("origin/{}...{source}", merge_request.target_branch),
    };
    let numstat = git(repo, &["diff", "--numstat", &range]).ok()?;
    let mut added = 0;
    let mut removed = 0;
    for line in numstat.lines() {
        let mut fields = line.split('\t');
        // Binary files show as `-`, which counts as nothing.
        added += fields.next()?.parse::<u64>().unwrap_or(0);
        removed += fields.next()?.parse::<u64>().unwrap_or(0);
    }
    Some((added, removed))
}

/// Fetches the branch, and its base for a stacked merge request, in one round
/// trip, then checks it out or fast-forwards it if it is already local. A
/// local branch that cannot fast-forward is kept as it is, with a warning.
/// The other worktree of `repo` that has `branch` checked out, if any.
pub fn worktree_holding(repo: &Path, branch: &str) -> Option<PathBuf> {
    let listing = git(repo, &["worktree", "list", "--porcelain"]).ok()?;
    let wanted = format!("branch refs/heads/{branch}");
    let mut path: Option<PathBuf> = None;
    for line in listing.lines() {
        if let Some(worktree) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(worktree));
        } else if line == wanted {
            let found = path.clone()?;
            let same = std::fs::canonicalize(&found).ok() == std::fs::canonicalize(repo).ok();
            return if same || !found.is_dir() {
                None
            } else {
                Some(found)
            };
        }
    }
    None
}

fn check_out(merge_request: &MergeRequest) -> Result<Option<String>> {
    let repo = &merge_request.repo;
    let source = merge_request.source_branch.as_str();
    let target = merge_request.target_branch.as_str();
    let current = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    // A dirty tree is fine when the branch is already the one checked out,
    // as in the user's own worktree: the diff then shows their work in
    // progress, which is said so it is not mistaken for the merge request's
    // diff. Switching branches over a dirty tree is not fine.
    let dirty_files = git(repo, &["status", "--porcelain"])?.lines().count();
    let mut warning = None;
    if dirty_files > 0 {
        if current != source {
            bail!(
                "the working tree at {} has uncommitted changes; commit or stash them first",
                repo.display()
            );
        }
        warning = Some(format!(
            "{dirty_files} uncommitted file{} in {}; the diff includes them, so it differs from GitLab",
            if dirty_files == 1 { "" } else { "s" },
            repo.display()
        ));
    }
    // Every remote branch is brought up to date: the diff is from the merge
    // base with the target, and the base rule looks at what the target
    // already has, so stale copies show merged work again. Should the full
    // fetch fail, the two branches that matter are tried on their own; the
    // target may have been deleted under a stacked merge request, in which
    // case GitLab's base commit, already in the source branch's history,
    // is used instead.
    if git(repo, &["fetch", "--prune", "origin"]).is_err()
        && git(repo, &["fetch", "origin", source, target]).is_err()
    {
        git(repo, &["fetch", "origin", source])?;
    } else if let Ok(project) = Project::discover(repo) {
        gitlab::mark_git_fetched(&project);
    }

    let remote = format!("origin/{source}");
    let local_exists = git(
        repo,
        &["show-ref", "--verify", &format!("refs/heads/{source}")],
    )
    .is_ok();
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
    Ok(warning)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sh(repo: &Path, args: &[&str]) -> String {
        git(repo, args).unwrap_or_else(|error| panic!("git {args:?}: {error:#}"))
    }

    fn commit(repo: &Path, file: &str, text: &str, message: &str) -> String {
        fs::write(repo.join(file), text).unwrap();
        sh(repo, &["add", "."]);
        sh(repo, &["commit", "-q", "-m", message]);
        sh(repo, &["rev-parse", "HEAD"])
    }

    fn merge_request(repo: &Path, source: &str, target: &str) -> MergeRequest {
        MergeRequest {
            iid: 1,
            title: String::new(),
            description: None,
            author: None,
            source_branch: source.into(),
            target_branch: target.into(),
            updated_at: String::new(),
            web_url: String::new(),
            repo: repo.to_path_buf(),
            depth: 0,
            stacked: target != "main",
            approved_by: Vec::new(),
            user_has_approved: false,
            additions: 0,
            deletions: 0,
            comment_count: 0,
            my_comment_count: 0,
            diff_refs: None,
            draft: false,
            mine: false,
            counted_sha: None,
            details_current: false,
        }
    }

    /// A stack of two branches whose lower one is squash-merged into main:
    /// the upper branch should be diffed from its own first commit, not
    /// from the merge base, which would show the merged work again.
    #[test]
    fn a_squash_merged_parent_moves_the_base_up_to_the_branches_own_commits() {
        let dir = std::env::temp_dir().join(format!("changewalk-base-{}", std::process::id()));
        let remote = dir.join("remote.git");
        let repo = dir.join("clone");
        fs::create_dir_all(&remote).unwrap();
        sh(&remote, &["init", "-q", "--bare", "-b", "main"]);
        sh(&dir, &["clone", "-q", remote.to_str().unwrap(), "clone"]);
        sh(&repo, &["config", "user.email", "t@example.com"]);
        sh(&repo, &["config", "user.name", "Test"]);
        sh(&repo, &["checkout", "-q", "-b", "main"]);
        commit(&repo, "a.txt", "base\n", "base");
        sh(&repo, &["checkout", "-q", "-b", "parent"]);
        commit(&repo, "b.txt", "parent work\n", "parent work");
        sh(&repo, &["checkout", "-q", "-b", "child"]);
        let child_commit = commit(&repo, "c.txt", "child work\n", "child work");
        // Squash-merge the parent into main, as GitLab does.
        sh(&repo, &["checkout", "-q", "main"]);
        sh(&repo, &["merge", "-q", "--squash", "parent"]);
        sh(&repo, &["commit", "-q", "-m", "parent squashed"]);
        sh(&repo, &["push", "-q", "origin", "main", "child", "parent"]);

        let base = review_base(&merge_request(&repo, "child", "main")).unwrap();
        assert_eq!(base, sh(&repo, &["rev-parse", &format!("{child_commit}^")]));

        // With nothing merged below it, a branch on main needs no base.
        sh(&repo, &["checkout", "-q", "-b", "plain", "main"]);
        commit(&repo, "d.txt", "plain\n", "plain");
        sh(&repo, &["push", "-q", "origin", "plain"]);
        assert_eq!(review_base(&merge_request(&repo, "plain", "main")), None);

        fs::remove_dir_all(&dir).ok();
    }
}
