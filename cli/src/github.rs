//! GitHub's half of the work, answering the same questions as
//! [`crate::gitlab`] so the rest of the tool cannot tell the two apart.
//!
//! A pull request is carried in GitLab's shape: its number becomes the
//! `iid`, `head` and `base` become the source and target branches, and the
//! body becomes the description. Two differences are worth knowing:
//!
//! - GitHub clears a review request once the review is submitted, so the
//!   list cannot be read off `requested_reviewers` alone; the numbers come
//!   from search, which also knows what the user has already reviewed.
//! - Approval is not a flag but the last of a reviewer's reviews, so a
//!   `CHANGES_REQUESTED` or `DISMISSED` after an `APPROVED` takes it back.

use crate::forge::{self, Author, Details, DiffRefs, Forge, MergeRequest, Project, Role};
use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};

/// `api.github.com` for github.com, and the `/api/v3` prefix a GitHub
/// Enterprise install serves the same API under.
fn api_root(host: &str) -> String {
    if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com".to_string()
    } else {
        format!("https://{host}/api/v3")
    }
}

fn graphql_url(host: &str) -> String {
    if host.eq_ignore_ascii_case("github.com") {
        "https://api.github.com/graphql".to_string()
    } else {
        format!("https://{host}/api/graphql")
    }
}

/// The token gh logged in with, or `GITHUB_TOKEN` or `GH_TOKEN`.
fn token(host: &str) -> Result<String> {
    forge::token_from_env_or_cli(
        Forge::Github,
        host,
        &["GITHUB_TOKEN", "GH_TOKEN"],
        &["auth", "token", "--hostname", host],
    )
}

/// Sets the headers every request to GitHub wants.
fn get(url: &str, token: &str) -> ureq::Request {
    forge::agent()
        .get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/vnd.github+json")
        .set("X-GitHub-Api-Version", "2022-11-28")
}

/// The logged-in user's login, cached since it never changes.
fn username(host: &str, token: &str) -> Result<String> {
    forge::username(host, || {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let user: User = get(&format!("{}/user", api_root(host)), token)
            .call()
            .context("asking GitHub who the token belongs to")?
            .into_json()
            .context("the user reply is not the expected JSON")?;
        Ok(user.login)
    })
}

#[derive(Deserialize)]
struct GitRef {
    #[serde(rename = "ref")]
    name: String,
    #[serde(default)]
    sha: String,
}

#[derive(Deserialize)]
struct Login {
    #[serde(default)]
    login: String,
}

/// A pull request as the REST API lists it.
#[derive(Deserialize)]
struct Pull {
    number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    user: Option<Login>,
    head: GitRef,
    base: GitRef,
    #[serde(default)]
    updated_at: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    draft: bool,
}

impl Pull {
    fn into_merge_request(self, username: &str) -> MergeRequest {
        let author = self.user.map(|user| Author {
            // The REST list carries only the login, and a login is what
            // GitHub itself shows, so it stands in for the name too.
            name: user.login.clone(),
            username: user.login,
        });
        let mine = author
            .as_ref()
            .is_some_and(|author| author.username == username);
        MergeRequest {
            iid: self.number,
            title: self.title,
            description: self.body,
            author,
            source_branch: self.head.name,
            target_branch: self.base.name,
            updated_at: self.updated_at,
            web_url: self.html_url,
            repo: Default::default(),
            forge: Forge::Github,
            depth: 0,
            stacked: false,
            approved_by: Vec::new(),
            user_has_approved: false,
            additions: 0,
            deletions: 0,
            comment_count: 0,
            my_comment_count: 0,
            diff_refs: Some(DiffRefs {
                base_sha: self.base.sha.clone(),
                head_sha: self.head.sha,
                start_sha: self.base.sha,
            }),
            draft: self.draft,
            mine,
            counted_sha: None,
            details_current: false,
        }
    }
}

pub fn fetch_merge_request(project: &Project, iid: u64) -> Result<MergeRequest> {
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let (owner, name) = project.owner_and_name()?;
    let pull: Pull = get(
        &format!(
            "{}/repos/{owner}/{name}/pulls/{iid}",
            api_root(&project.host)
        ),
        &token,
    )
    .call()
    .map_err(|error| {
        anyhow!(
            "fetching #{iid} from {}: {}",
            project.path,
            forge::describe(&error)
        )
    })?
    .into_json()
    .context("the pull request reply is not the expected JSON")?;
    Ok(pull.into_merge_request(&username))
}

/// The numbers of the open pull requests matching one search qualifier,
/// for example `review-requested:octocat`.
fn search_numbers(project: &Project, token: &str, qualifier: &str) -> Result<HashSet<u64>> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        items: Vec<Item>,
    }
    #[derive(Deserialize)]
    struct Item {
        number: u64,
    }
    let query = format!("repo:{} is:pr is:open {qualifier}", project.path);
    let reply: Reply = get(&format!("{}/search/issues", api_root(&project.host)), token)
        .query("q", &query)
        .query("per_page", "100")
        // Required since GitHub retired the old issue-search syntax.
        .query("advanced_search", "true")
        .call()
        .map_err(|error| {
            anyhow!(
                "searching {} for {qualifier}: {}",
                project.path,
                forge::describe(&error)
            )
        })?
        .into_json()
        .context("the search reply is not the expected JSON")?;
    Ok(reply.items.into_iter().map(|item| item.number).collect())
}

/// The pull requests the user reviews or is assigned, plus their own.
///
/// Which pull requests those are comes from search, because GitHub drops a
/// reviewer from `requested_reviewers` the moment they submit a review and
/// a review half-done is exactly the one worth listing. The bodies then
/// come from one listing of the open pull requests, with a single request
/// for any wanted number the first hundred did not reach.
pub fn fetch_lists(project: &Project, role: Role) -> Result<Vec<MergeRequest>> {
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let (owner, name) = project.owner_and_name()?;

    let mut wanted = match role {
        Role::Reviewer => {
            let mut requested =
                search_numbers(project, &token, &format!("review-requested:{username}"))?;
            requested.extend(search_numbers(
                project,
                &token,
                &format!("reviewed-by:{username}"),
            )?);
            requested
        }
        Role::Assignee => search_numbers(project, &token, &format!("assignee:{username}"))?,
    };
    wanted.extend(search_numbers(
        project,
        &token,
        &format!("author:{username}"),
    )?);
    if wanted.is_empty() {
        return Ok(Vec::new());
    }

    let listed: Vec<Pull> = get(
        &format!("{}/repos/{owner}/{name}/pulls", api_root(&project.host)),
        &token,
    )
    .query("state", "open")
    .query("per_page", "100")
    .query("sort", "updated")
    .query("direction", "desc")
    .call()
    .map_err(|error| {
        anyhow!(
            "listing pull requests for {}: {}",
            project.path,
            forge::describe(&error)
        )
    })?
    .into_json()
    .context("the pull request list is not the expected JSON")?;

    let mut merge_requests: Vec<MergeRequest> = listed
        .into_iter()
        .filter(|pull| wanted.contains(&pull.number))
        .map(|pull| pull.into_merge_request(&username))
        .collect();
    // A repository busy enough to have more than a hundred open pull
    // requests may not have listed one the user reviews; ask for those by
    // number rather than leave a review off the list.
    for iid in wanted {
        if !merge_requests.iter().any(|mr| mr.iid == iid) {
            merge_requests.push(fetch_merge_request(project, iid)?);
        }
    }
    Ok(merge_requests)
}

/// Line counts, comment counts, and approvals for every listed pull
/// request, in one GraphQL query per project, each pull request under its
/// own alias.
pub fn fetch_details(project: &Project, iids: &[u64]) -> Result<HashMap<u64, Details>> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        data: Option<Data>,
        #[serde(default)]
        errors: Vec<GraphqlError>,
    }
    #[derive(Deserialize)]
    struct GraphqlError {
        #[serde(default)]
        message: String,
    }
    #[derive(Deserialize)]
    struct Data {
        repository: Option<HashMap<String, Option<Node>>>,
    }
    #[derive(Deserialize)]
    struct Node {
        number: u64,
        #[serde(default)]
        additions: u64,
        #[serde(default)]
        deletions: u64,
        #[serde(default)]
        comments: Option<Comments>,
        #[serde(default)]
        reviews: Option<Reviews>,
    }
    #[derive(Deserialize)]
    struct Comments {
        #[serde(rename = "totalCount", default)]
        total_count: u64,
        #[serde(default)]
        nodes: Vec<Commenter>,
    }
    #[derive(Deserialize)]
    struct Commenter {
        #[serde(default)]
        author: Option<GraphqlAuthor>,
    }
    #[derive(Deserialize)]
    struct GraphqlAuthor {
        #[serde(default)]
        login: String,
        /// Only a user has a name; a bot does not.
        #[serde(default)]
        name: Option<String>,
    }
    #[derive(Deserialize)]
    struct Reviews {
        #[serde(default)]
        nodes: Vec<Review>,
    }
    #[derive(Deserialize)]
    struct Review {
        #[serde(default)]
        state: String,
        #[serde(default)]
        author: Option<GraphqlAuthor>,
        #[serde(rename = "bodyText", default)]
        body_text: String,
        #[serde(default)]
        comments: Option<Comments>,
    }
    if iids.is_empty() {
        return Ok(HashMap::new());
    }
    let token = token(&project.host)?;
    let username = username(&project.host, &token)?;
    let (owner, name) = project.owner_and_name()?;
    let fields = "number additions deletions \
         comments(first: 100) { totalCount nodes { author { login } } } \
         reviews(first: 100) { nodes { state bodyText author { login ... on User { name } } \
         comments(first: 100) { totalCount nodes { author { login } } } } }";
    let selections: Vec<String> = iids
        .iter()
        .map(|iid| format!("p{iid}: pullRequest(number: {iid}) {{ {fields} }}"))
        .collect();
    let query = format!(
        "{{ repository(owner: \"{owner}\", name: \"{name}\") {{ {} }} }}",
        selections.join(" ")
    );
    let reply: Reply = forge::agent()
        .post(&graphql_url(&project.host))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/vnd.github+json")
        .send_json(serde_json::json!({ "query": query }))
        .map_err(|error| {
            anyhow!(
                "fetching details for {}: {}",
                project.path,
                forge::describe(&error)
            )
        })?
        .into_json()
        .context("the diff stats reply is not the expected JSON")?;
    let repository = reply.data.and_then(|data| data.repository);
    let Some(repository) = repository else {
        let message = reply
            .errors
            .first()
            .map(|error| error.message.clone())
            .unwrap_or_else(|| "GitHub returned no data".to_string());
        bail!("fetching diff stats for {}: {message}", project.path);
    };
    Ok(repository
        .into_values()
        .flatten()
        .map(|node| {
            let reviews = node
                .reviews
                .map(|reviews| reviews.nodes)
                .unwrap_or_default();
            let verdicts: Vec<Verdict> = reviews
                .iter()
                .filter_map(|review| {
                    let author = review.author.as_ref()?;
                    Some(Verdict {
                        login: author.login.clone(),
                        name: author
                            .name
                            .as_deref()
                            .map(str::trim)
                            .filter(|name| !name.is_empty())
                            .unwrap_or(&author.login)
                            .to_string(),
                        state: review.state.clone(),
                    })
                })
                .collect();
            let (approved_by, user_has_approved) = approvals(&verdicts, &username);
            let issue_comments = node.comments.as_ref();
            let review_comment_count: u64 = reviews
                .iter()
                .map(|review| {
                    review
                        .comments
                        .as_ref()
                        .map_or(0, |comments| comments.total_count)
                })
                .sum();
            // A review with something written on it is a comment too, as
            // GitLab's note count would have it.
            let review_body_count = reviews
                .iter()
                .filter(|review| !review.body_text.trim().is_empty())
                .count() as u64;
            let mine = |author: &Option<GraphqlAuthor>| {
                author
                    .as_ref()
                    .is_some_and(|author| author.login == username)
            };
            let my_comment_count = issue_comments.map_or(0, |comments| {
                comments
                    .nodes
                    .iter()
                    .filter(|comment| mine(&comment.author))
                    .count() as u64
            }) + reviews
                .iter()
                .map(|review| {
                    let inline = review.comments.as_ref().map_or(0, |comments| {
                        comments
                            .nodes
                            .iter()
                            .filter(|comment| mine(&comment.author))
                            .count() as u64
                    });
                    let body =
                        u64::from(!review.body_text.trim().is_empty() && mine(&review.author));
                    inline + body
                })
                .sum::<u64>();
            (
                node.number,
                Details {
                    additions: node.additions,
                    deletions: node.deletions,
                    comment_count: issue_comments.map_or(0, |comments| comments.total_count)
                        + review_comment_count
                        + review_body_count,
                    my_comment_count: Some(my_comment_count),
                    approved_by,
                    user_has_approved,
                },
            )
        })
        .collect())
}

/// One review's verdict, as both the query above and the test below can
/// describe it.
struct Verdict {
    login: String,
    name: String,
    state: String,
}

/// Who has approved, by their last review that says anything about it:
/// GitHub lets a reviewer approve, then ask for changes, and only the
/// latest of the two counts. A comment-only review is no verdict at all.
/// Reviews arrive oldest first.
fn approvals(verdicts: &[Verdict], username: &str) -> (Vec<String>, bool) {
    let mut latest: Vec<(&str, &str, bool)> = Vec::new();
    for verdict in verdicts {
        if !matches!(
            verdict.state.as_str(),
            "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED"
        ) {
            continue;
        }
        let entry = (
            verdict.login.as_str(),
            verdict.name.as_str(),
            verdict.state == "APPROVED",
        );
        match latest.iter_mut().find(|(login, _, _)| *login == entry.0) {
            Some(known) => *known = entry,
            None => latest.push(entry),
        }
    }
    let user_has_approved = latest
        .iter()
        .any(|(login, _, approved)| *approved && *login == username);
    let approved_by = latest
        .into_iter()
        .filter(|(_, _, approved)| *approved)
        .map(|(_, name, _)| name.to_string())
        .collect();
    (approved_by, user_has_approved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review(login: &str, state: &str) -> Verdict {
        Verdict {
            login: login.into(),
            name: login.into(),
            state: state.into(),
        }
    }

    /// Only a reviewer's last verdict counts, and a comment-only review is
    /// no verdict at all.
    #[test]
    fn the_last_review_of_each_reviewer_decides_approval() {
        let reviews = [
            review("ada", "APPROVED"),
            review("bob", "CHANGES_REQUESTED"),
            review("ada", "CHANGES_REQUESTED"),
            review("cleo", "COMMENTED"),
            review("bob", "APPROVED"),
            review("dee", "APPROVED"),
            review("dee", "COMMENTED"),
        ];
        let (approved_by, mine) = approvals(&reviews, "bob");
        assert_eq!(approved_by, vec!["bob".to_string(), "dee".to_string()]);
        assert!(mine);
        assert!(!approvals(&reviews, "ada").1);
        assert!(!approvals(&reviews, "cleo").1);
        assert_eq!(approvals(&[], "ada"), (Vec::new(), false));
    }

    /// The one translation that matters: GitHub's words for a pull
    /// request, read into the shape the rest of the tool and the Zed fork
    /// expect.
    #[test]
    fn a_pull_request_reads_as_a_merge_request() {
        let json = r###"{
            "number": 1628,
            "title": "Read a reply as a result",
            "body": "## Why\n\nBecause.",
            "user": { "login": "ada" },
            "head": { "ref": "ada/reply", "sha": "aaa111" },
            "base": { "ref": "main", "sha": "bbb222" },
            "updated_at": "2026-10-19T10:00:00Z",
            "html_url": "https://github.com/owner/repo/pull/1628",
            "draft": true,
            "requested_reviewers": [{ "login": "bob" }]
        }"###;
        let pull: Pull = serde_json::from_str(json).unwrap();
        let mine = pull.into_merge_request("ada");
        assert_eq!(mine.iid, 1628);
        assert_eq!(mine.number(), "#1628");
        assert_eq!(mine.source_branch, "ada/reply");
        assert_eq!(mine.target_branch, "main");
        assert_eq!(mine.description.as_deref(), Some("## Why\n\nBecause."));
        assert_eq!(mine.author_name(), "ada");
        assert_eq!(mine.forge, Forge::Github);
        assert!(mine.draft);
        assert!(mine.mine);
        let refs = mine.diff_refs.unwrap();
        assert_eq!(
            (refs.base_sha.as_str(), refs.head_sha.as_str()),
            ("bbb222", "aaa111")
        );

        // The same pull request seen by someone else is not theirs.
        let theirs: Pull = serde_json::from_str(json).unwrap();
        assert!(!theirs.into_merge_request("bob").mine);
    }

    /// Asks GitHub itself, so the query and the way its reply is read are
    /// checked against the real thing rather than a fixture that can drift
    /// from it. A merged pull request is used because its counts no longer
    /// move. Needs a login, so it is not part of a plain `cargo test`:
    ///
    /// ```text
    /// cargo test -- --ignored
    /// ```
    #[test]
    #[ignore = "asks GitHub; needs `gh auth login` or a GITHUB_TOKEN"]
    fn live_details_read_back_what_github_reports() {
        let project = Project {
            repo: std::path::PathBuf::from("."),
            forge: Forge::Github,
            host: "github.com".into(),
            path: "zed-industries/zed".into(),
            default_branch: "main".into(),
        };
        let details = fetch_details(&project, &[63036]).unwrap();
        let merged = details.get(&63036).expect("the pull request was asked for");
        assert_eq!((merged.additions, merged.deletions), (2, 2));
        // Four comments on the pull request, and a review that approved it
        // without writing anything, so it adds nothing to the count.
        assert_eq!(merged.comment_count, 4);
        assert_eq!(merged.approved_by, vec!["Marshall Bowers".to_string()]);
        assert!(!merged.user_has_approved);

        // And the single pull request, through the dispatcher, which must
        // attach the clone and host the backend leaves empty. A launch runs
        // `git` in `repo`, so an empty one ran git in no directory at all.
        let one = crate::forge::fetch_merge_request(&project, 63036).unwrap();
        assert_eq!(one.number(), "#63036");
        assert_eq!(one.target_branch, "main");
        assert!(!one.title.is_empty());
        assert_eq!(one.repo, project.repo);
        assert_eq!(one.forge, Forge::Github);
    }

    #[test]
    fn enterprise_hosts_serve_the_api_under_a_prefix() {
        assert_eq!(api_root("github.com"), "https://api.github.com");
        assert_eq!(
            api_root("github.corp.example"),
            "https://github.corp.example/api/v3"
        );
        assert_eq!(graphql_url("github.com"), "https://api.github.com/graphql");
        assert_eq!(
            graphql_url("github.corp.example"),
            "https://github.corp.example/api/graphql"
        );
    }
}
