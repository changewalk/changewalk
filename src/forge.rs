//! What GitLab and GitHub have in common: the change under review, the
//! clone it belongs to, where the list is cached, and how changes stack on
//! each other. The two hosts differ only in how a list is fetched, which
//! [`gitlab`](crate::gitlab) and [`github`](crate::github) each answer; the
//! functions at the bottom of this file pick between them.
//!
//! Both hosts are described in GitLab's words — a merge request with an
//! `iid` — because that is what the Zed fork reads from the file a launch
//! writes. A pull request is carried in the same shape, and [`Forge`] holds
//! the words to show it under.

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Which host a clone's origin points at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Forge {
    #[default]
    Gitlab,
    Github,
}

impl Forge {
    pub const ALL: [Forge; 2] = [Forge::Gitlab, Forge::Github];

    pub fn name(self) -> &'static str {
        match self {
            Forge::Gitlab => "GitLab",
            Forge::Github => "GitHub",
        }
    }

    /// `!` on GitLab, `#` on GitHub, as each writes a change's number.
    pub fn number_prefix(self) -> &'static str {
        match self {
            Forge::Gitlab => "!",
            Forge::Github => "#",
        }
    }

    /// `!1628` or `#1628`.
    pub fn number(self, iid: u64) -> String {
        format!("{}{iid}", self.number_prefix())
    }

    /// What the host calls one change, capitalised and plural.
    pub fn plural_noun(self) -> &'static str {
        match self {
            Forge::Gitlab => "Merge requests",
            Forge::Github => "Pull requests",
        }
    }

    /// What the host calls one change, lower case and singular.
    pub fn noun(self) -> &'static str {
        match self {
            Forge::Gitlab => "merge request",
            Forge::Github => "pull request",
        }
    }

    /// The command-line tool that holds a login for this host.
    pub fn cli(self) -> &'static str {
        match self {
            Forge::Gitlab => "glab",
            Forge::Github => "gh",
        }
    }

    /// How the host names itself in a remote, for example `gitlab.com` or
    /// `gitlab.example.com`.
    fn matches_host(self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        match self {
            Forge::Gitlab => host.contains("gitlab"),
            Forge::Github => host.contains("github"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Role {
    Reviewer,
    Assignee,
}

/// A local clone and the project its origin points at.
#[derive(Clone, Debug)]
pub struct Project {
    pub repo: PathBuf,
    pub forge: Forge,
    pub host: String,
    /// The project path, for example `group/sub/project` or `owner/repo`.
    pub path: String,
    pub default_branch: String,
}

/// The commits the host's own diff of the change is between.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct DiffRefs {
    pub base_sha: String,
    pub head_sha: String,
    pub start_sha: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Author {
    pub name: String,
    #[serde(default)]
    pub username: String,
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
    /// The clone the change belongs to, set after loading.
    #[serde(default)]
    pub repo: PathBuf,
    /// The host the change came from, so the list can use its words.
    #[serde(default)]
    pub forge: Forge,
    /// How deep the change sits in a stack: 0 targets the default branch,
    /// 1 targets another listed change's branch, and so on.
    #[serde(skip)]
    pub depth: usize,
    /// True when the target is not the default branch, whether or not the
    /// parent change is in the list.
    #[serde(skip)]
    pub stacked: bool,
    /// Who has approved, by name. A list reply does not carry this; it
    /// comes from the details request.
    #[serde(default)]
    pub approved_by: Vec<String>,
    #[serde(default)]
    pub user_has_approved: bool,
    /// Lines added and removed, from the details request.
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    /// Comments on the change, system notes left out, and how many of them
    /// are the user's.
    #[serde(default)]
    pub comment_count: u64,
    #[serde(default)]
    pub my_comment_count: u64,
    /// Only a single-change reply carries this; a list does not.
    #[serde(default)]
    pub diff_refs: Option<DiffRefs>,
    #[serde(default)]
    pub draft: bool,
    /// The user opened this change themselves.
    #[serde(default)]
    pub mine: bool,
    /// The tip of the branch when the local line counts were taken, so
    /// they are only retaken when it moves.
    #[serde(default)]
    pub counted_sha: Option<String>,
    /// The approvals and counts came from the cache and need no request,
    /// because the change has not been updated since.
    #[serde(skip)]
    pub details_current: bool,
}

impl MergeRequest {
    pub fn author_name(&self) -> &str {
        self.author
            .as_ref()
            .map_or("", |author| author.name.as_str())
    }

    /// `!1628` or `#1628`, as the change's own host writes it.
    pub fn number(&self) -> String {
        self.forge.number(self.iid)
    }
}

impl Project {
    /// Reads the origin remote. A host that does not name itself is taken
    /// to be `fallback`, which onboarding asked the user for, since a
    /// self-hosted GitLab or GitHub Enterprise can be called anything.
    pub fn discover(repo: &Path, fallback: Forge) -> Result<Self> {
        let url = git(repo, &["remote", "get-url", "origin"])
            .with_context(|| format!("{} has no origin remote", repo.display()))?;
        let (host, path) =
            parse_remote(&url).with_context(|| format!("{url} does not look like a git remote"))?;
        let forge = Forge::ALL
            .into_iter()
            .find(|forge| forge.matches_host(&host))
            .unwrap_or(fallback);
        let default_branch = git(
            repo,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
        .ok()
        .and_then(|name| name.strip_prefix("origin/").map(str::to_string))
        .unwrap_or_else(|| "main".to_string());
        Ok(Self {
            repo: repo.to_path_buf(),
            forge,
            host,
            path,
            default_branch,
        })
    }

    /// `owner` and `repo`, for the hosts whose API wants them apart.
    pub fn owner_and_name(&self) -> Result<(&str, &str)> {
        self.path
            .rsplit_once('/')
            .with_context(|| format!("{} is not an owner/name path", self.path))
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
        .with_context(|| format!("running git in {}", repo.display()))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn cache_dir() -> Option<PathBuf> {
    let dir = dirs::cache_dir()?.join("changewalk");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The change most recently opened in the editor from the picker, kept
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

/// One agent for the whole run, so connections and their TLS handshakes
/// are reused between requests.
pub fn agent() -> ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT
        .get_or_init(|| {
            ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(20))
                .timeout(Duration::from_secs(60))
                .build()
        })
        .clone()
}

/// A token from the first of `vars` that is set in the environment, else
/// what `cli` prints for `host`.
///
/// Held for the rest of the run once found: a refresh asks for a token at
/// each of its stages and for each project, and reading one back out of
/// the system keyring is neither instant nor silent.
pub fn token_from_env_or_cli(
    forge: Forge,
    host: &str,
    vars: &[&str],
    cli_args: &[&str],
) -> Result<String> {
    static TOKENS: std::sync::OnceLock<std::sync::Mutex<HashMap<(Forge, String), String>>> =
        std::sync::OnceLock::new();
    let tokens = TOKENS.get_or_init(Default::default);
    let key = (forge, host.to_string());
    if let Ok(held) = tokens.lock()
        && let Some(token) = held.get(&key)
    {
        return Ok(token.clone());
    }
    let token = ask_for_token(forge, host, vars, cli_args)?;
    if let Ok(mut held) = tokens.lock() {
        held.insert(key, token.clone());
    }
    Ok(token)
}

fn ask_for_token(forge: Forge, host: &str, vars: &[&str], cli_args: &[&str]) -> Result<String> {
    for var in vars {
        if let Some(token) = std::env::var_os(var) {
            let token = token.to_string_lossy().trim().to_string();
            if !token.is_empty() {
                return Ok(token);
            }
        }
    }
    let cli = forge.cli();
    let output = Command::new(cli).args(cli_args).output().with_context(|| {
        format!(
            "running {cli}; install it and run `{cli} auth login`, or set {}",
            vars.join(" or ")
        )
    })?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || token.is_empty() {
        bail!(
            "no {} token for {host}; run `{cli} auth login` or set {}",
            forge.name(),
            vars.join(" or ")
        );
    }
    Ok(token)
}

#[derive(Deserialize, Serialize)]
struct CachedUser {
    host: String,
    username: String,
}

/// The logged-in user's name, cached since it never changes. `ask` runs
/// only on the first call for a host.
pub fn username(host: &str, ask: impl FnOnce() -> Result<String>) -> Result<String> {
    let file = cache_dir().map(|dir| dir.join(format!("user-{host}.json")));
    if let Some(file) = &file
        && let Ok(text) = std::fs::read_to_string(file)
        && let Ok(user) = serde_json::from_str::<CachedUser>(&text)
        && user.host == host
    {
        return Ok(user.username);
    }
    let username = ask()?;
    if let Some(file) = file {
        let cached = CachedUser {
            host: host.to_string(),
            username: username.clone(),
        };
        if let Ok(text) = serde_json::to_string(&cached) {
            std::fs::write(file, text).ok();
        }
    }
    Ok(username)
}

/// What the details request tells about a change beyond the list.
pub struct Details {
    pub additions: u64,
    pub deletions: u64,
    pub comment_count: u64,
    pub my_comment_count: u64,
    pub approved_by: Vec<String>,
    pub user_has_approved: bool,
}

/// How long a `git fetch` is trusted before a refresh runs another.
const GIT_FETCH_FRESH_FOR: Duration = Duration::from_secs(120);

fn git_fetch_file(project: &Project) -> Option<PathBuf> {
    let name = format!(
        "fetched-{}__{}",
        project.host,
        project.path.replace('/', "__")
    );
    Some(cache_dir()?.join(name))
}

/// Notes that the project's clone was just fetched.
pub fn mark_git_fetched(project: &Project) {
    if let Some(file) = git_fetch_file(project) {
        std::fs::write(file, now_seconds().to_string()).ok();
    }
}

/// Whether the clone was fetched recently enough to leave alone.
pub fn git_fetched_recently(project: &Project) -> bool {
    git_fetch_file(project)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .is_some_and(|at| now_seconds().saturating_sub(at) < GIT_FETCH_FRESH_FOR.as_secs())
}

#[derive(Deserialize, Serialize)]
struct Cache {
    fetched_at: u64,
    merge_requests: Vec<MergeRequest>,
}

/// Remembers `merge_requests` as the project's list for the next start.
pub fn write_cache(project: &Project, merge_requests: &[MergeRequest]) {
    let Some(file) = project.cache_file() else {
        return;
    };
    let cache = Cache {
        fetched_at: now_seconds(),
        merge_requests: merge_requests.to_vec(),
    };
    if let Ok(text) = serde_json::to_string(&cache) {
        std::fs::write(file, text).ok();
    }
}

/// The changes from the last fetch, if any, so the list shows at once.
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
            merge_request.forge = project.forge;
            merge_request
        })
        .collect()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// One change as the host has it now, with its diff refs. Used at launch
/// so a retargeted or rebased change is diffed against the right base,
/// whatever the cached list said.
///
/// The clone and host the change belongs to are the project's; a host
/// reply does not carry them, so they are set here, at the one seam both
/// backends pass through. A launch runs `git` in `repo`, so leaving it
/// empty would run git in no directory at all.
pub fn fetch_merge_request(project: &Project, iid: u64) -> Result<MergeRequest> {
    let mut merge_request = match project.forge {
        Forge::Gitlab => crate::gitlab::fetch_merge_request(project, iid),
        Forge::Github => crate::github::fetch_merge_request(project, iid),
    }?;
    merge_request.repo = project.repo.clone();
    merge_request.forge = project.forge;
    merge_request.stacked = merge_request.target_branch != project.default_branch;
    Ok(merge_request)
}

/// The open changes the user reviews or is assigned, and the ones they
/// opened themselves, drafts included, newest first. Approvals and counts
/// are carried over from `cached` for any change the host has not updated
/// since, so the details stage can skip it.
pub fn fetch_lists(
    project: &Project,
    role: Role,
    cached: &[MergeRequest],
) -> Result<Vec<MergeRequest>> {
    let mut merge_requests = match project.forge {
        Forge::Gitlab => crate::gitlab::fetch_lists(project, role)?,
        Forge::Github => crate::github::fetch_lists(project, role)?,
    };
    merge_requests.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    for merge_request in &mut merge_requests {
        merge_request.repo = project.repo.clone();
        merge_request.forge = project.forge;
        merge_request.stacked = merge_request.target_branch != project.default_branch;
        if let Some(known) = cached.iter().find(|known| {
            known.iid == merge_request.iid && known.updated_at == merge_request.updated_at
        }) {
            merge_request.approved_by = known.approved_by.clone();
            merge_request.user_has_approved = known.user_has_approved;
            merge_request.additions = known.additions;
            merge_request.deletions = known.deletions;
            merge_request.comment_count = known.comment_count;
            merge_request.my_comment_count = known.my_comment_count;
            merge_request.counted_sha = known.counted_sha.clone();
            merge_request.details_current = true;
        }
    }
    Ok(merge_requests)
}

/// Fills in approvals, line counts, and comment counts for the changes
/// that still need them, in one query per project.
pub fn fill_details(project: &Project, merge_requests: &mut [MergeRequest]) -> Result<()> {
    let iids: Vec<u64> = merge_requests
        .iter()
        .filter(|mr| !mr.details_current)
        .map(|mr| mr.iid)
        .collect();
    if iids.is_empty() {
        return Ok(());
    }
    let details = match project.forge {
        Forge::Gitlab => crate::gitlab::fetch_details(project, &iids)?,
        Forge::Github => crate::github::fetch_details(project, &iids)?,
    };
    for merge_request in merge_requests.iter_mut() {
        if let Some(details) = details.get(&merge_request.iid) {
            merge_request.additions = details.additions;
            merge_request.deletions = details.deletions;
            merge_request.comment_count = details.comment_count;
            merge_request.my_comment_count = details.my_comment_count;
            merge_request.approved_by = details.approved_by.clone();
            merge_request.user_has_approved = details.user_has_approved;
            merge_request.counted_sha = None;
        }
    }
    Ok(())
}

/// Orders the list so each change follows the one it is stacked on,
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
            forge: Forge::Gitlab,
            depth: 0,
            stacked: false,
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
        assert_eq!(
            parse_remote("git@github.com:owner/repo.git"),
            Some(("github.com".into(), "owner/repo".into()))
        );
        assert_eq!(parse_remote("nonsense"), None);
    }

    /// A host that names itself picks its own forge; anything else falls
    /// back to what onboarding was told.
    #[test]
    fn hosts_that_name_themselves_pick_their_own_forge() {
        let forge = |host: &str, fallback: Forge| {
            Forge::ALL
                .into_iter()
                .find(|forge| forge.matches_host(host))
                .unwrap_or(fallback)
        };
        assert_eq!(forge("github.com", Forge::Gitlab), Forge::Github);
        assert_eq!(forge("GitHub.MyCorp.com", Forge::Gitlab), Forge::Github);
        assert_eq!(forge("gitlab.example.com", Forge::Github), Forge::Gitlab);
        assert_eq!(forge("git.example.com", Forge::Github), Forge::Github);
        assert_eq!(forge("git.example.com", Forge::Gitlab), Forge::Gitlab);
    }

    #[test]
    fn each_forge_writes_a_number_its_own_way() {
        assert_eq!(Forge::Gitlab.number(12), "!12");
        assert_eq!(Forge::Github.number(12), "#12");
    }
}
