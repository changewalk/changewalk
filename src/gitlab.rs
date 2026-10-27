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
    /// Who has approved, by name. The list reply does not carry this; it
    /// comes from a second request per merge request.
    #[serde(default)]
    pub approved_by: Vec<String>,
    #[serde(default)]
    pub user_has_approved: bool,
    /// Lines added and removed, from a second request per project.
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
}

impl MergeRequest {
    pub fn author_name(&self) -> &str {
        self.author
            .as_ref()
            .map_or("", |author| author.name.as_str())
    }
}

impl Project {
    pub fn discover(repo: &Path) -> Result<Self> {
        let url = git(repo, &["remote", "get-url", "origin"])
            .with_context(|| format!("{} has no origin remote", repo.display()))?;
        let (host, path) = parse_remote(&url)
            .with_context(|| format!("{url} does not look like a GitLab remote"))?;
        let default_branch = git(
            repo,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
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
    let dir = dirs::cache_dir()?.join("changewalk");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The merge request most recently opened in Zed from the picker, kept
/// between runs so the list can point it out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LastOpened {
    pub web_url: String,
    /// RFC 3339, local time.
    pub at: String,
}

fn last_opened_file() -> Option<PathBuf> {
    Some(cache_dir()?.join("last_opened.json"))
}

pub fn last_opened() -> Option<LastOpened> {
    let text = std::fs::read_to_string(last_opened_file()?).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn remember_opened(merge_request: &MergeRequest) -> LastOpened {
    let opened = LastOpened {
        web_url: merge_request.web_url.clone(),
        at: chrono::Local::now().to_rfc3339(),
    };
    if let Some(file) = last_opened_file()
        && let Ok(text) = serde_json::to_string(&opened)
    {
        std::fs::write(file, text).ok();
    }
    opened
}

/// The token glab logged in with, or `GITLAB_TOKEN`.
fn token(host: &str) -> Result<String> {
    if let Some(token) = std::env::var_os("GITLAB_TOKEN") {
        return Ok(token.to_string_lossy().into_owned());
    }
    let output = Command::new("glab")
        .args(["config", "get", "token", "--host", host])
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
    // Approvals take one request each, so fetch them side by side. A
    // failure leaves that merge request showing no approvals.
    let iids: Vec<u64> = merge_requests.iter().map(|mr| mr.iid).collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = merge_requests
            .iter()
            .map(|merge_request| {
                let token = token.as_str();
                let endpoint =
                    project.api(&format!("merge_requests/{}/approvals", merge_request.iid));
                scope.spawn(move || fetch_approvals(&endpoint, token))
            })
            .collect();
        // Line counts come from one GraphQL query for the whole list.
        let diff_stats = scope.spawn(|| fetch_diff_stats(project, &token, &iids));
        for (merge_request, handle) in merge_requests.iter_mut().zip(handles) {
            if let Ok(Ok(approvals)) = handle.join() {
                merge_request.approved_by = approvals.approved_by;
                merge_request.user_has_approved = approvals.user_has_approved;
            }
        }
        if let Ok(Ok(diff_stats)) = diff_stats.join() {
            for merge_request in &mut merge_requests {
                if let Some((additions, deletions)) = diff_stats.get(&merge_request.iid) {
                    merge_request.additions = *additions;
                    merge_request.deletions = *deletions;
                }
            }
        }
    });
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

struct Approvals {
    approved_by: Vec<String>,
    user_has_approved: bool,
}

/// Lines added and removed by each merge request, by iid.
fn fetch_diff_stats(
    project: &Project,
    token: &str,
    iids: &[u64],
) -> Result<HashMap<u64, (u64, u64)>> {
    #[derive(Deserialize)]
    struct Reply {
        data: Option<Data>,
    }
    #[derive(Deserialize)]
    struct Data {
        project: Option<ProjectNode>,
    }
    #[derive(Deserialize)]
    struct ProjectNode {
        #[serde(rename = "mergeRequests")]
        merge_requests: Nodes,
    }
    #[derive(Deserialize)]
    struct Nodes {
        nodes: Vec<Node>,
    }
    #[derive(Deserialize)]
    struct Node {
        iid: String,
        #[serde(rename = "diffStatsSummary")]
        diff_stats_summary: Option<Summary>,
    }
    #[derive(Deserialize)]
    struct Summary {
        additions: u64,
        deletions: u64,
    }
    if iids.is_empty() {
        return Ok(HashMap::new());
    }
    let iids: Vec<String> = iids.iter().map(|iid| format!("\"{iid}\"")).collect();
    let query = format!(
        "{{ project(fullPath: \"{}\") {{ mergeRequests(iids: [{}]) {{ nodes {{ iid diffStatsSummary {{ additions deletions }} }} }} }} }}",
        project.path,
        iids.join(",")
    );
    let reply: Reply = agent()
        .post(&format!("https://{}/api/graphql", project.host))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(serde_json::json!({ "query": query }))
        .map_err(|error| anyhow!("fetching diff stats: {error}"))?
        .into_json()
        .context("the diff stats reply is not the expected JSON")?;
    Ok(reply
        .data
        .and_then(|data| data.project)
        .map(|project| project.merge_requests.nodes)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|node| {
            let summary = node.diff_stats_summary?;
            Some((
                node.iid.parse().ok()?,
                (summary.additions, summary.deletions),
            ))
        })
        .collect())
}

fn fetch_approvals(endpoint: &str, token: &str) -> Result<Approvals> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        approved_by: Vec<Approver>,
        #[serde(default)]
        user_has_approved: bool,
    }
    #[derive(Deserialize)]
    struct Approver {
        #[serde(default)]
        user: Option<Author>,
    }
    let reply: Reply = agent()
        .get(endpoint)
        .set("PRIVATE-TOKEN", token)
        .call()
        .map_err(|error| anyhow!("fetching approvals: {error}"))?
        .into_json()
        .context("the approvals reply is not the expected JSON")?;
    Ok(Approvals {
        approved_by: reply
            .approved_by
            .into_iter()
            .filter_map(|approver| approver.user.map(|user| user.name))
            .collect(),
        user_has_approved: reply.user_has_approved,
    })
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
            .get(&(
                merge_request.repo.clone(),
                merge_request.target_branch.clone(),
            ))
            .copied()
            .filter(|&parent| parent != index);
        match parent {
            Some(parent) => children.entry(parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    let newest_first = |indices: &mut Vec<usize>| {
        indices.sort_by(|&a, &b| {
            merge_requests[b]
                .updated_at
                .cmp(&merge_requests[a].updated_at)
        })
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
            approved_by: Vec::new(),
            user_has_approved: false,
            additions: 0,
            deletions: 0,
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
