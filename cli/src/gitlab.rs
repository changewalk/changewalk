//! GitLab's half of the work: where a merge request is fetched from, and
//! the one GraphQL query that fills in approvals and counts. Everything
//! these return is shaped by [`crate::forge`], which also decides when to
//! call them.

use crate::forge::{self, Details, Forge, MergeRequest, Project, Role};
use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;
use std::collections::HashMap;

impl Project {
    fn api(&self, endpoint: &str) -> String {
        format!(
            "https://{}/api/v4/projects/{}/{endpoint}",
            self.host,
            urlencoding::encode(&self.path)
        )
    }
}

/// The token glab logged in with, or `GITLAB_TOKEN`.
fn token(host: &str) -> Result<String> {
    forge::token_from_env_or_cli(
        Forge::Gitlab,
        host,
        &["GITLAB_TOKEN"],
        &["config", "get", "token", "--host", host],
    )
}

/// The logged-in user's name, cached since it never changes.
fn username(host: &str, token: &str) -> Result<String> {
    forge::username(host, || {
        #[derive(Deserialize)]
        struct User {
            username: String,
        }
        let user: User = forge::agent()
            .get(&format!("https://{host}/api/v4/user"))
            .set("PRIVATE-TOKEN", token)
            .call()
            .context("asking GitLab who the token belongs to")?
            .into_json()
            .context("the user reply is not the expected JSON")?;
        Ok(user.username)
    })
}

pub fn fetch_merge_request(project: &Project, iid: u64) -> Result<MergeRequest> {
    let token = token(&project.host)?;
    forge::agent()
        .get(&project.api(&format!("merge_requests/{iid}")))
        .set("PRIVATE-TOKEN", &token)
        .call()
        .map_err(|error| anyhow!("fetching !{iid} from {}: {error}", project.path))?
        .into_json()
        .context("the merge request reply is not the expected JSON")
}

/// The merge requests the user reviews or is assigned, plus their own.
/// `mine` is set here; the shared layer sorts and fills the rest in.
pub fn fetch_lists(project: &Project, role: Role) -> Result<Vec<MergeRequest>> {
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let filter = match role {
        Role::Reviewer => "reviewer_username",
        Role::Assignee => "assignee_username",
    };
    let list = |filter: &str| -> Result<Vec<MergeRequest>> {
        forge::agent()
            .get(&project.api("merge_requests"))
            .query("state", "opened")
            .query("scope", "all")
            .query("per_page", "100")
            .query(filter, &username)
            .set("PRIVATE-TOKEN", &token)
            .call()
            .map_err(|error| anyhow!("listing merge requests for {}: {error}", project.path))?
            .into_json()
            .context("the merge request list is not the expected JSON")
    };
    let mut merge_requests = list(filter)?;
    for own in list("author_username")? {
        if !merge_requests.iter().any(|mr| mr.iid == own.iid) {
            merge_requests.push(own);
        }
    }
    for merge_request in &mut merge_requests {
        merge_request.mine = merge_request
            .author
            .as_ref()
            .is_some_and(|author| author.username == username);
    }
    Ok(merge_requests)
}

/// Line counts, comment counts, and approvals for every listed merge
/// request, in one GraphQL query per project. The user's own comments are
/// counted among the first hundred notes, which covers all but the longest
/// threads.
pub fn fetch_details(project: &Project, iids: &[u64]) -> Result<HashMap<u64, Details>> {
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
        #[serde(rename = "userNotesCount", default)]
        user_notes_count: u64,
        #[serde(default)]
        notes: Option<Notes>,
        #[serde(rename = "approvedBy", default)]
        approved_by: Option<Approvers>,
    }
    #[derive(Deserialize)]
    struct Approvers {
        #[serde(default)]
        nodes: Vec<Approver>,
    }
    #[derive(Deserialize)]
    struct Approver {
        #[serde(default)]
        name: String,
        #[serde(default)]
        username: String,
    }
    #[derive(Deserialize)]
    struct Summary {
        additions: u64,
        deletions: u64,
    }
    #[derive(Deserialize)]
    struct Notes {
        #[serde(default)]
        nodes: Vec<Note>,
    }
    #[derive(Deserialize)]
    struct Note {
        #[serde(default)]
        system: bool,
        #[serde(default)]
        author: Option<NoteAuthor>,
    }
    #[derive(Deserialize)]
    struct NoteAuthor {
        #[serde(default)]
        username: String,
    }
    if iids.is_empty() {
        return Ok(HashMap::new());
    }
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let iids: Vec<String> = iids.iter().map(|iid| format!("\"{iid}\"")).collect();
    let query = format!(
        "{{ project(fullPath: \"{}\") {{ mergeRequests(iids: [{}]) {{ nodes {{ iid userNotesCount diffStatsSummary {{ additions deletions }} approvedBy {{ nodes {{ name username }} }} notes(first: 100) {{ nodes {{ system author {{ username }} }} }} }} }} }} }}",
        project.path,
        iids.join(",")
    );
    let reply: Reply = forge::agent()
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
            let (additions, deletions) = node
                .diff_stats_summary
                .map(|summary| (summary.additions, summary.deletions))
                .unwrap_or_default();
            let my_comment_count = node
                .notes
                .as_ref()
                .map(|notes| {
                    notes
                        .nodes
                        .iter()
                        .filter(|note| {
                            !note.system
                                && note
                                    .author
                                    .as_ref()
                                    .is_some_and(|author| author.username == username)
                        })
                        .count() as u64
                })
                .unwrap_or_default();
            let approvers = node
                .approved_by
                .map(|approvers| approvers.nodes)
                .unwrap_or_default();
            let user_has_approved = approvers
                .iter()
                .any(|approver| approver.username == username);
            Some((
                node.iid.parse().ok()?,
                Details {
                    additions,
                    deletions,
                    comment_count: node.user_notes_count,
                    my_comment_count,
                    approved_by: approvers
                        .into_iter()
                        .map(|approver| approver.name)
                        .collect(),
                    user_has_approved,
                },
            ))
        })
        .collect())
}
