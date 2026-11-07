//! Pick an open merge or pull request you review, and open its diff in Zed
//! with a suggested review order, guiding notes, and the change's
//! description above the files.

mod config;
mod forge;
mod github;
mod gitlab;
mod launch;
mod onboarding;
mod picker;
mod warm;

use anyhow::{Context as _, Result};
use clap::Parser;
use forge::{MergeRequest, Project, Role};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// Pick a merge or pull request you review and open it in Zed in review
/// order.
///
/// Arrow keys move, Enter checks the branch out and opens Zed while the
/// list stays open, `g` opens the change on GitLab or GitHub, `r` refreshes
/// the list, `u` and `d` scroll the description, `q` quits. Stacked changes
/// sit under the one they are stacked on and are diffed against that branch
/// rather than the default branch.
///
/// The first run asks which host you review on and which editor to read
/// reviews in; `--onboard` asks again.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// List changes assigned to you instead of ones you review.
    #[arg(long)]
    assignee: bool,

    /// Ask again which host and editor to use, then exit.
    #[arg(long)]
    onboard: bool,

    /// The Zed app bundle to open. Defaults to the one onboarding found.
    #[arg(long, env = "ZED_APP")]
    zed_app: Option<PathBuf>,

    /// Open this change in Zed straight away, without the list.
    #[arg(long, value_name = "NUMBER")]
    open: Option<u64>,

    /// Write the Zed reading order for this change and exit.
    #[arg(long, value_name = "NUMBER")]
    warm: Option<u64>,

    /// Repositories to list changes from. Defaults to $CHANGEWALK_REPOS
    /// (colon separated), else the current directory.
    repos: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = onboarding::configure(args.onboard)?;
    if args.onboard {
        return Ok(());
    }
    let role = if args.assignee {
        Role::Assignee
    } else {
        Role::Reviewer
    };
    let projects = repositories(args.repos)?
        .iter()
        .map(|repo| Project::discover(repo, config.forge))
        .collect::<Result<Vec<_>>>()?;
    let default_branches: HashMap<PathBuf, String> = projects
        .iter()
        .map(|project| (project.repo.clone(), project.default_branch.clone()))
        .collect();

    // The last list shows straight away from the cache; `r` asks the host
    // for a fresh one.
    let cached: Vec<MergeRequest> = projects
        .iter()
        .flat_map(forge::cached_merge_requests)
        .collect();
    let title = projects
        .iter()
        .map(|project| project.path.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let app = args.zed_app.unwrap_or_else(|| config.app());
    if let Some(iid) = args.warm {
        let (project, merge_request) = projects
            .iter()
            .find_map(|project| {
                forge::fetch_merge_request(project, iid)
                    .ok()
                    .map(|mr| (project, mr))
            })
            .with_context(|| format!("no change {iid} in the listed projects"))?;
        let written = warm::warm_by_iid(project, &merge_request)?;
        println!(
            "{}",
            if written {
                "wrote the order"
            } else {
                "an order was already cached, or the branch is not fetched"
            }
        );
        return Ok(());
    }
    if let Some(iid) = args.open {
        let merge_request = cached
            .iter()
            .find(|mr| mr.iid == iid)
            .cloned()
            .or_else(|| {
                projects
                    .iter()
                    .find_map(|project| forge::fetch_merge_request(project, iid).ok())
            })
            .with_context(|| format!("no change {iid} in the listed projects"))?;
        println!("{}", launch::open_in_zed(&merge_request, &app)?);
        return Ok(());
    }
    picker::run(
        cached,
        projects,
        role,
        default_branches,
        title,
        move |merge_request| launch::open_in_zed(merge_request, &app),
    )
}

fn repositories(args: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    let repos = if !args.is_empty() {
        args
    } else if let Some(configured) = std::env::var_os("CHANGEWALK_REPOS")
        .or_else(|| std::env::var_os("DIFFWALK_REPOS"))
        .or_else(|| std::env::var_os("ZED_REVIEW_REPOS"))
    {
        configured
            .to_string_lossy()
            .split(':')
            .filter(|path| !path.is_empty())
            .map(expand_home)
            .collect()
    } else {
        vec![std::env::current_dir().context("reading the current directory")?]
    };
    repos
        .into_iter()
        .map(|repo| {
            repo.canonicalize()
                .with_context(|| format!("repository {} does not exist", repo.display()))
        })
        .collect()
}

pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => Path::new(path).to_path_buf(),
    }
}
