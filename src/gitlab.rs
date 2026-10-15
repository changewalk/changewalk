//! GitLab merge requests: where to find them, how to fetch and cache them,
//! and how they stack on each other.

use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// A local clone and the GitLab project its origin points at.
#[derive(Clone, Debug)]
pub struct Project {
    pub repo: PathBuf,
    pub host: String,
    /// The project path, for example `group/sub/project`.
    pub path: String,
    pub default_branch: String,
}

#[derive(Clone, Copy, Debug)]
pub enum Role {
    Reviewer,
    Assignee,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Author {
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MergeRequest {
    pub iid: u64,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<Author>,
    pub source_branch: String,
    pub target_branch: String,
    #[serde(default)]
    pub updated_at: String,
    pub web_url: String,
    /// The clone the merge request belongs to, set after loading.
    #[serde(default)]
    pub repo: PathBuf,
    /// How deep the merge request sits in a stack: 0 targets the default
    /// branch, 1 targets another listed merge request's branch, and so on.
    #[serde(skip)]
    pub depth: usize,
    /// True when the target is not the default branch, whether or not the
    /// parent merge request is in the list.
    #[serde(skip)]
    pub stacked: bool,
}

impl MergeRequest {
    pub fn author_name(&self) -> &str {
        self.author.as_ref().map_or("", |author| author.name.as_str())
    }

    pub fn updated_day(&self) -> &str {
        self.updated_at.get(..10).unwrap_or(&self.updated_at)
    }
}

impl Project {
    pub fn discover(repo: &Path) -> Result<Self> {
        let url = git(repo, &["remote", "get-url", "origin"])
            .with_context(|| format!("{} has no origin remote", repo.display()))?;
        let (host, path) = parse_remote(&url)
            .with_context(|| format!("{url} does not look like a GitLab remote"))?;
        let default_branch = git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
            .ok()
            .and_then(|name| name.strip_prefix("origin/").map(str::to_string))
            .unwrap_or_else(|| "main".to_string());
        Ok(Self {
            repo: repo.to_path_buf(),
            host,
            path,
            default_branch,
        })
    }

    fn api(&self, endpoint: &str) -> String {
        format!(
            "https://{}/api/v4/projects/{}/{endpoint}",
            self.host,
            urlencoding::encode(&self.path)
        )
    }

    fn cache_file(&self) -> Option<PathBuf> {
        let name = format!("{}__{}.json", self.host, self.path.replace('/', "__"));
        Some(cache_dir()?.join(name))
    }
}

/// `git@host:group/project.git` or `https://host/group/project.git`.
fn parse_remote(url: &str) -> Option<(String, String)> {
    let rest = url.trim();
    let (host, path) = if let Some(rest) = rest.strip_prefix("git@") {
        rest.split_once(':')?
    } else {
        let rest = rest.split_once("://")?.1;
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        rest.split_once('/')?
    };
    let path = path.trim_end_matches('/').trim_end_matches(".git");
    (!path.is_empty()).then(|| (host.to_string(), path.to_string()))
}

pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn cache_dir() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("zed-review");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The token glab logged in with, or `GITLAB_TOKEN`.
fn token(host: &str) -> Result<String> {
    if let Some(token) = std::env::var_os("GITLAB_TOKEN") {
        return Ok(token.to_string_lossy().into_owned());
    }
    let output = Command::new("glab")
        .args(["config", "get", "token", "-h", host])
        .output()
        .context("running glab; install it and run `glab auth login`")?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || token.is_empty() {
        bail!("no GitLab token for {host}; run `glab auth login` or set GITLAB_TOKEN");
    }
    Ok(token)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout(Duration::from_secs(60))
        .build()
}

#[derive(Deserialize, Serialize)]
struct CachedUser {
    host: String,
    username: String,
}

/// The logged-in user's name, cached since it never changes.
fn username(host: &str, token: &str) -> Result<String> {
    let file = cache_dir().map(|dir| dir.join(format!("user-{host}.json")));
    if let Some(file) = &file
        && let Ok(text) = std::fs::read_to_string(file)
        && let Ok(user) = serde_json::from_str::<CachedUser>(&text)
        && user.host == host
    {
        return Ok(user.username);
    }
    #[derive(Deserialize)]
    struct User {
        username: String,
    }
    let user: User = agent()
        .get(&format!("https://{host}/api/v4/user"))
        .set("PRIVATE-TOKEN", token)
        .call()
        .context("asking GitLab who the token belongs to")?
        .into_json()
        .context("the user reply is not the expected JSON")?;
    if let Some(file) = file {
        let cached = CachedUser {
            host: host.to_string(),
            username: user.username.clone(),
        };
        std::fs::write(file, serde_json::to_string(&cached)?).ok();
    }
    Ok(user.username)
}

/// The open merge requests the user reviews or is assigned, newest first,
/// and writes them to the cache.
pub fn fetch_merge_requests(project: &Project, role: Role) -> Result<Vec<MergeRequest>> {
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let filter = match role {
        Role::Reviewer => "reviewer_username",
        Role::Assignee => "assignee_username",
    };
    let mut merge_requests: Vec<MergeRequest> = agent()
        .get(&project.api("merge_requests"))
        .query("state", "opened")
        .query("scope", "all")
        .query("per_page", "100")
        .query(filter, &username)
        .set("PRIVATE-TOKEN", &token)
        .call()
        .map_err(|error| anyhow!("listing merge requests for {}: {error}", project.path))?
        .into_json()
        .context("the merge request list is not the expected JSON")?;
    for merge_request in &mut merge_requests {
        merge_request.repo = project.repo.clone();
    }
    if let Some(file) = project.cache_file() {
        let cache = Cache {
            fetched_at: now_seconds(),
            merge_requests: merge_requests.clone(),
        };
        std::fs::write(file, serde_json::to_string(&cache)?).ok();
    }
    Ok(merge_requests)
}

#[derive(Deserialize, Serialize)]
struct Cache {
    fetched_at: u64,
    merge_requests: Vec<MergeRequest>,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The merge requests from the last fetch, if any, so the list shows at once.
pub fn cached_merge_requests(project: &Project) -> Vec<MergeRequest> {
    let Some(file) = project.cache_file() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<Cache>(&text) else {
        return Vec::new();
    };
    cache
        .merge_requests
        .into_iter()
        .map(|mut merge_request| {
            merge_request.repo = project.repo.clone();
            merge_request
        })
        .collect()
}

/// Orders the list so each merge request follows the one it is stacked on,
/// newest stacks first, and sets the depth of each.
pub fn order_as_stacks(
    merge_requests: Vec<MergeRequest>,
    default_branches: &HashMap<PathBuf, String>,
) -> Vec<MergeRequest> {
    let mut merge_requests = merge_requests;
    for merge_request in &mut merge_requests {
        let default_branch = default_branches
            .get(&merge_request.repo)
            .map_or("main", String::as_str);
        merge_request.stacked = merge_request.target_branch != default_branch;
    }
    let index_by_source: HashMap<(PathBuf, String), usize> = merge_requests
        .iter()
        .enumerate()
        .map(|(index, mr)| ((mr.repo.clone(), mr.source_branch.clone()), index))
        .collect();

    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (index, merge_request) in merge_requests.iter().enumerate() {
        let parent = index_by_source
            .get(&(merge_request.repo.clone(), merge_request.target_branch.clone()))
            .copied()
            .filter(|&parent| parent != index);
        match parent {
            Some(parent) => children.entry(parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    let newest_first = |indices: &mut Vec<usize>| {
        indices.sort_by(|&a, &b| merge_requests[b].updated_at.cmp(&merge_requests[a].updated_at))
    };
    newest_first(&mut roots);
    for indices in children.values_mut() {
        newest_first(indices);
    }

    let mut ordered = Vec::with_capacity(merge_requests.len());
    let mut seen = vec![false; merge_requests.len()];
    fn visit(
        index: usize,
        depth: usize,
        children: &HashMap<usize, Vec<usize>>,
        seen: &mut [bool],
        ordered: &mut Vec<(usize, usize)>,
    ) {
        if seen[index] {
            return;
        }
        seen[index] = true;
        ordered.push((index, depth));
        for &child in children.get(&index).into_iter().flatten() {
            visit(child, depth + 1, children, seen, ordered);
        }
    }
    for root in roots {
        visit(root, 0, &children, &mut seen, &mut ordered);
    }
    // A cycle, or a stack whose root is missing, is still listed.
    for index in 0..merge_requests.len() {
        visit(index, 0, &children, &mut seen, &mut ordered);
    }

    let mut by_index: Vec<Option<MergeRequest>> = merge_requests.into_iter().map(Some).collect();
    ordered
        .into_iter()
        .filter_map(|(index, depth)| {
            let mut merge_request = by_index[index].take()?;
            merge_request.depth = depth;
            Some(merge_request)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mr(iid: u64, source: &str, target: &str, updated: &str) -> MergeRequest {
        MergeRequest {
            iid,
            title: format!("!{iid}"),
            description: None,
            author: None,
            source_branch: source.into(),
            target_branch: target.into(),
            updated_at: updated.into(),
            web_url: String::new(),
            repo: PathBuf::from("/r"),
            depth: 0,
            stacked: false,
        }
    }

    #[test]
    fn stacks_follow_their_parent_newest_first() {
        let defaults = HashMap::from([(PathBuf::from("/r"), "main".to_string())]);
        let ordered = order_as_stacks(
            vec![
                mr(1, "a", "main", "2026-01-01"),
                mr(2, "b", "a", "2026-01-02"),
                mr(3, "c", "b", "2026-01-03"),
                mr(4, "d", "main", "2026-01-04"),
                mr(5, "e", "gone", "2026-01-05"),
            ],
            &defaults,
        );
        let summary: Vec<(u64, usize, bool)> = ordered
            .iter()
            .map(|mr| (mr.iid, mr.depth, mr.stacked))
            .collect();
        assert_eq!(
            summary,
            vec![
                (5, 0, true),
                (4, 0, false),
                (1, 0, false),
                (2, 1, true),
                (3, 2, true),
            ]
        );
    }

    #[test]
    fn remotes_parse_to_host_and_path() {
        assert_eq!(
            parse_remote("git@gitlab.com:group/sub/project.git"),
            Some(("gitlab.com".into(), "group/sub/project".into()))
        );
        assert_eq!(
            parse_remote("https://oauth2:token@gitlab.example.com/group/project/"),
            Some(("gitlab.example.com".into(), "group/project".into()))
        );
        assert_eq!(parse_remote("nonsense"), None);
    }
}
