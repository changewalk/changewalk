//! Warms Zed's review order cache for the merge requests at the top of
//! the list, so opening one shows its reading order at once instead of
//! waiting for the model.
//!
//! This mirrors what the Zed fork does when it has no cached order: the
//! same prompts, the same file excerpts, the same parsing, and the same
//! cache file, keyed by clone, base, and commit. The prompt files are
//! copies of the fork's; keep them in step.

use crate::gitlab::{MergeRequest, Project, git};
use crate::launch;
use anyhow::{Context as _, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha1::{Digest as _, Sha1};
use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

const ORDER_EXCERPT_BYTES_PER_FILE: usize = 800;
const ORDER_EXCERPT_BYTES_TOTAL: usize = 16_000;
const MAX_EXCERPT_BYTES_PER_FILE: usize = 6_000;
const MAX_EXCERPT_BYTES_TOTAL: usize = 60_000;
const MAX_CONTEXT_BYTES: usize = 6_000;
const MAX_NOTES_PER_FILE: usize = 3;

/// The models the fork's defaults amount to; override with
/// `CHANGEWALK_ORDER_MODEL` and `CHANGEWALK_NOTES_MODEL`.
const DEFAULT_ORDER_MODEL: &str = "claude-haiku-5-5";
const DEFAULT_NOTES_MODEL: &str = "claude-sonnet-5-5";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Note {
    line: u32,
    text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Entry {
    path: String,
    reason: String,
    notes: Vec<Note>,
}

#[derive(Serialize, Deserialize)]
struct CachedOrder {
    entries: Vec<Entry>,
}

/// Where the Zed fork keeps its review cache.
fn cache_dir() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/Application Support/Zed")
            .join("review_cache"),
    )
}

/// The cache file for a diff of `head_sha` against `base_ref` in the clone
/// at `work_dir`, as the fork names it.
fn cache_file(work_dir: &Path, base_ref: &str, head_sha: &str) -> Option<PathBuf> {
    let key = format!("{}\n{base_ref}\n{head_sha}", work_dir.display());
    let hash = format!("{:x}", Sha1::digest(key.as_bytes()));
    Some(cache_dir()?.join(format!("order-{hash}.json")))
}

/// Writes an order for each of the first `limit` merge requests that are
/// not the user's own and have none cached. Returns a line per merge
/// request warmed, for the footer. Without `ANTHROPIC_API_KEY` it does
/// nothing.
pub fn warm_order_cache(
    project: &Project,
    merge_requests: &[MergeRequest],
    limit: usize,
) -> Vec<String> {
    let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") else {
        return Vec::new();
    };
    let mut warmed = Vec::new();
    for merge_request in merge_requests.iter().filter(|mr| !mr.mine).take(limit) {
        match warm_one(project, merge_request, &api_key) {
            Ok(true) => warmed.push(format!("warmed the order for !{}", merge_request.iid)),
            Ok(false) => {}
            Err(error) => warmed.push(format!(
                "could not warm the order for !{}: {error:#}",
                merge_request.iid
            )),
        }
    }
    warmed
}

/// Warms one merge request by iid, for `changewalk --warm`.
pub fn warm_by_iid(project: &Project, merge_request: &MergeRequest) -> Result<bool> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is not set")?;
    warm_one(project, merge_request, &api_key)
}

/// True when an order was written; false when one was already there or
/// the branch is not fetched.
fn warm_one(project: &Project, merge_request: &MergeRequest, api_key: &str) -> Result<bool> {
    let mut merge_request = merge_request.clone();
    if let Some(worktree) =
        launch::worktree_holding(&merge_request.repo, &merge_request.source_branch)
    {
        merge_request.repo = worktree;
    }
    let repo = merge_request.repo.clone();
    let source = format!("origin/{}", merge_request.source_branch);
    let Ok(head_sha) = git(&repo, &["rev-parse", &source]) else {
        return Ok(false);
    };
    let base_ref = launch::review_base(&merge_request)
        .unwrap_or_else(|| format!("origin/{}", project.default_branch));
    let Some(file) = cache_file(&repo, &base_ref, &head_sha) else {
        return Ok(false);
    };
    if file.exists() {
        return Ok(false);
    }

    let files = changed_files(&repo, &base_ref, &head_sha)?;
    if files.is_empty() {
        return Ok(false);
    }
    let known: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
    let context = format!(
        "Title: {}\nDescription:\n{}\n",
        merge_request.title,
        merge_request.description.as_deref().unwrap_or("").trim()
    );
    let pinned = order_from_description(merge_request.description.as_deref().unwrap_or(""), &known);

    let order_model =
        std::env::var("CHANGEWALK_ORDER_MODEL").unwrap_or_else(|_| DEFAULT_ORDER_MODEL.into());
    let notes_model =
        std::env::var("CHANGEWALK_NOTES_MODEL").unwrap_or_else(|_| DEFAULT_NOTES_MODEL.into());
    let order_reply = ask(
        api_key,
        &order_model,
        &build_prompt(
            include_str!("review_order_prompt.txt"),
            &context,
            &files,
            ORDER_EXCERPT_BYTES_PER_FILE,
            ORDER_EXCERPT_BYTES_TOTAL,
        ),
    )?;
    let order_entries = parse_entries(&order_reply, &known)?;
    if order_entries.is_empty() {
        bail!("the order reply names no file from this diff");
    }
    let mut entries = order_entries
        .into_iter()
        .map(|(path, raw)| Entry {
            path,
            reason: raw.reason.trim().to_string(),
            notes: notes_of(raw),
        })
        .collect::<Vec<_>>();
    entries = with_pinned_first(entries, &pinned);

    let notes_reply = ask(
        api_key,
        &notes_model,
        &build_prompt(
            include_str!("review_notes_prompt.txt"),
            &context,
            &files,
            MAX_EXCERPT_BYTES_PER_FILE,
            MAX_EXCERPT_BYTES_TOTAL,
        ),
    )?;
    // Notes are the smaller half: an order without them still saves the
    // wait, so a bad notes reply is reported but does not stop the write.
    let mut notes: HashMap<String, Vec<Note>> = match parse_entries(&notes_reply, &known) {
        Ok(entries) => entries
            .into_iter()
            .map(|(path, raw)| (path, notes_of(raw)))
            .collect(),
        Err(error) => {
            eprintln!("notes for !{}: {error:#}", merge_request.iid);
            HashMap::new()
        }
    };
    for entry in &mut entries {
        entry.notes = notes.remove(&entry.path).unwrap_or_default();
    }

    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&file, serde_json::to_string(&CachedOrder { entries })?)
        .with_context(|| format!("writing {}", file.display()))?;
    Ok(true)
}

/// One changed file as the prompts describe it.
struct ChangedFile {
    path: String,
    status: &'static str,
    added_lines: usize,
    removed_lines: usize,
    /// `-` and `+` lines, the added ones numbered, capped per file.
    excerpt: String,
}

/// The files `base_ref..head_sha` changes, in path order, with their
/// changed lines as the fork's prompts show them.
fn changed_files(repo: &Path, base_ref: &str, head_sha: &str) -> Result<Vec<ChangedFile>> {
    let range = format!("{base_ref}..{head_sha}");
    let statuses = git(repo, &["diff", "--name-status", "--no-renames", &range])?;
    let mut files = Vec::new();
    for line in statuses.lines() {
        let mut fields = line.split('\t');
        let (Some(status), Some(path)) = (fields.next(), fields.next()) else {
            continue;
        };
        let status = match status.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            Some('U') => "conflicted",
            _ => "modified",
        };
        let patch = git(repo, &["diff", "-U0", &range, "--", path])?;
        let (excerpt, added_lines, removed_lines) = excerpt_of(&patch);
        files.push(ChangedFile {
            path: path.to_string(),
            status,
            added_lines,
            removed_lines,
            excerpt,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// The changed lines of a unified diff with no context, as `-     |text`
/// and `+   42 |text`, with the counts of each.
fn excerpt_of(patch: &str) -> (String, usize, usize) {
    let mut excerpt = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut new_line = 0usize;
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: the new side starts at c.
            new_line = header
                .split_whitespace()
                .nth(1)
                .and_then(|plus| plus.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok())
                .unwrap_or(0);
            continue;
        }
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if let Some(text) = line.strip_prefix('-') {
            removed += 1;
            if excerpt.len() < MAX_EXCERPT_BYTES_PER_FILE {
                writeln!(excerpt, "-     |{text}").ok();
            }
        } else if let Some(text) = line.strip_prefix('+') {
            added += 1;
            if excerpt.len() < MAX_EXCERPT_BYTES_PER_FILE {
                writeln!(excerpt, "+{new_line:>5} |{text}").ok();
            }
            new_line += 1;
        }
    }
    (excerpt, added, removed)
}

fn build_prompt(
    template: &str,
    context: &str,
    files: &[ChangedFile],
    per_file_budget: usize,
    total_budget: usize,
) -> String {
    let mut prompt = template.to_string();
    let context = context.trim();
    if !context.is_empty() {
        let context = truncate_to_lines(context, MAX_CONTEXT_BYTES);
        write!(prompt, "\nAbout the merge request:\n{context}\n").ok();
    }
    writeln!(prompt, "\nChanged files ({}):", files.len()).ok();
    let mut budget = total_budget;
    for file in files {
        writeln!(
            prompt,
            "\n### {} ({}, +{} -{})",
            file.path, file.status, file.added_lines, file.removed_lines
        )
        .ok();
        if budget == 0 || file.excerpt.is_empty() {
            continue;
        }
        let excerpt = truncate_to_lines(&file.excerpt, budget.min(per_file_budget));
        budget = budget.saturating_sub(excerpt.len());
        writeln!(prompt, "```diff\n{excerpt}\n```").ok();
    }
    prompt
}

fn truncate_to_lines(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text.trim_end();
    }
    // The limit may fall inside a multi-byte character; back up to the
    // start of it before looking for the line break.
    let mut boundary = max_bytes;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let cut = text[..boundary].rfind('\n').unwrap_or(0);
    text[..cut].trim_end()
}

/// One round trip to the Anthropic messages API, returning the text.
/// Thinking is turned off, since the replies are JSON lists and thinking
/// spends the token budget; models differ in how it is turned off, so the
/// other form is tried when the first is refused.
fn ask(api_key: &str, model: &str, prompt: &str) -> Result<String> {
    match ask_with_thinking(api_key, model, prompt, "disabled") {
        Err(error) if error.to_string().contains("between_tools") => {
            ask_with_thinking(api_key, model, prompt, "between_tools")
        }
        result => result,
    }
}

fn ask_with_thinking(api_key: &str, model: &str, prompt: &str, thinking: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        content: Vec<Content>,
        #[serde(default)]
        stop_reason: Option<String>,
    }
    #[derive(Deserialize)]
    struct Content {
        #[serde(default)]
        text: String,
    }
    let reply: Reply = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(180))
        .build()
        .post("https://api.anthropic.com/v1/messages")
        .set("x-api-key", api_key)
        .set("anthropic-version", "2023-06-01")
        .send_json(serde_json::json!({
            "model": model,
            "max_tokens": 16_000,
            "thinking": {"type": thinking},
            "messages": [{"role": "user", "content": prompt}],
        }))
        .map_err(|error| match error {
            ureq::Error::Status(code, response) => {
                let body = response.into_string().unwrap_or_default();
                let body: String = body.chars().take(300).collect();
                anyhow!("asking {model}: status {code}: {body}")
            }
            other => anyhow!("asking {model}: {other}"),
        })?
        .into_json()
        .context("the model reply is not the expected JSON")?;
    if reply.stop_reason.as_deref() == Some("max_tokens") {
        bail!("{model} ran out of tokens before finishing the list");
    }
    let text: String = reply
        .content
        .into_iter()
        .map(|content| content.text)
        .collect();
    if text.trim().is_empty() {
        bail!("{model} replied with no text");
    }
    Ok(text)
}

#[derive(Deserialize)]
struct RawEntry {
    path: String,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    notes: Vec<RawNote>,
}

#[derive(Deserialize)]
struct RawNote {
    #[serde(default)]
    line: u32,
    #[serde(default)]
    note: String,
}

fn parse_entries(text: &str, known: &[String]) -> Result<Vec<(String, RawEntry)>> {
    let preview: String = text.chars().take(160).collect();
    let start = text
        .find('[')
        .with_context(|| format!("the reply has no JSON array: {preview:?}"))?;
    let end = text
        .rfind(']')
        .with_context(|| format!("the reply has no JSON array: {preview:?}"))?;
    if end < start {
        bail!("the reply has no JSON array");
    }
    let raw: Vec<RawEntry> = serde_json::from_str(&strip_trailing_commas(&text[start..=end]))
        .context("the reply is not a JSON array")?;
    let mut seen = HashSet::new();
    let entries: Vec<(String, RawEntry)> = raw
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path.trim().trim_start_matches("./").to_string();
            known.contains(&path).then_some(())?;
            seen.insert(path.clone()).then_some((path, entry))
        })
        .collect();
    if entries.is_empty() && !raw_is_empty(text) {
        bail!("the reply names no file from this diff: {preview:?}");
    }
    Ok(entries)
}

/// Whether the reply's array is `[]`: nothing to say, which is allowed for
/// notes.
fn raw_is_empty(text: &str) -> bool {
    let start = text.find('[').unwrap_or(0);
    let end = text.rfind(']').unwrap_or(text.len());
    text[start..end.max(start)]
        .trim_start_matches('[')
        .trim()
        .is_empty()
}

/// JSON with any comma that sits right before a `]` or `}` removed, since
/// models write lists that way and the parser refuses them. Commas inside
/// strings are left alone.
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = text.chars().collect();
    for (index, &c) in chars.iter().enumerate() {
        if in_string {
            out.push(c);
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => in_string = false,
                _ => escaped = false,
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            ',' => {
                let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
                if !matches!(next, Some(']') | Some('}')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn notes_of(entry: RawEntry) -> Vec<Note> {
    let mut lines = HashSet::new();
    entry
        .notes
        .into_iter()
        .filter(|note| note.line > 0 && !note.note.trim().is_empty())
        .filter(|note| lines.insert(note.line))
        .take(MAX_NOTES_PER_FILE)
        .map(|note| Note {
            line: note.line,
            text: note.note.trim().to_string(),
        })
        .collect()
}

/// `pinned` first, in their order, then the rest; a pinned reason wins
/// unless empty.
fn with_pinned_first(entries: Vec<Entry>, pinned: &[Entry]) -> Vec<Entry> {
    let mut ordered: Vec<Entry> = pinned
        .iter()
        .map(|pin| {
            let from_model = entries.iter().find(|entry| entry.path == pin.path);
            Entry {
                path: pin.path.clone(),
                reason: if pin.reason.is_empty() {
                    from_model
                        .map(|entry| entry.reason.clone())
                        .unwrap_or_default()
                } else {
                    pin.reason.clone()
                },
                notes: from_model
                    .map(|entry| entry.notes.clone())
                    .unwrap_or_default(),
            }
        })
        .collect();
    ordered.extend(
        entries
            .into_iter()
            .filter(|entry| !pinned.iter().any(|pin| pin.path == entry.path)),
    );
    ordered
}

/// Everything from here down is the fork's reading of a "review order"
/// in a merge request description, ported as is.
fn order_from_description(description: &str, known: &[String]) -> Vec<Entry> {
    let Some(section) = order_section(description) else {
        return Vec::new();
    };
    // Each backtick token that names a file starts an entry; the text up to
    // the next such token is the author's reason for it.
    let mut entries: Vec<Entry> = Vec::new();
    let mut reason = String::new();
    let mut rest = section;
    while let Some(start) = rest.find('`') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('`') else {
            break;
        };
        let token = &after[..end];
        let between = &rest[..start];
        match resolve_path(token, known) {
            Some(path) if !entries.iter().any(|entry| entry.path == path) => {
                if let Some(last) = entries.last_mut() {
                    reason.push_str(between);
                    last.reason = clean_reason(&reason);
                }
                reason.clear();
                entries.push(Entry {
                    path,
                    reason: String::new(),
                    notes: Vec::new(),
                });
            }
            _ => {
                reason.push_str(between);
                reason.push_str(token);
            }
        }
        rest = &after[end + 1..];
    }
    if let Some(last) = entries.last_mut() {
        reason.push_str(rest);
        last.reason = clean_reason(&reason);
    }
    entries
}

fn order_section(description: &str) -> Option<&str> {
    let lower = description.to_ascii_lowercase();
    let start = [
        "review order",
        "reading order",
        "read in this order",
        "suggested order",
    ]
    .iter()
    .filter_map(|phrase| lower.find(phrase))
    .min()?;
    let section = &description[start..];
    // A paragraph ends at a blank line. A heading such as "## Review order"
    // names no file itself, so the blank line after it does not end the
    // section; the list below it does.
    let mut end = section.len();
    let mut seen_text_line = false;
    let mut offset = 0;
    for line in section.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if seen_text_line {
                end = offset;
                break;
            }
        } else if offset > 0 && trimmed.starts_with('#') {
            end = offset;
            break;
        } else if offset > 0 || trimmed.contains('`') {
            seen_text_line = true;
        }
        offset += line.len();
    }
    Some(&section[..end])
}

fn resolve_path(token: &str, known: &[String]) -> Option<String> {
    let token = token.trim().trim_start_matches("./");
    if token.is_empty() || !token.contains('.') {
        return None;
    }
    let matches: Vec<&String> = if let Some((prefix, suffix)) = token.split_once("...") {
        known
            .iter()
            .filter(|path| {
                let path = path.as_str();
                path.starts_with(prefix.trim_end_matches('/'))
                    && path.ends_with(suffix.trim_start_matches('/'))
            })
            .collect()
    } else {
        let exact: Vec<&String> = known.iter().filter(|path| path.as_str() == token).collect();
        if !exact.is_empty() {
            exact
        } else {
            known
                .iter()
                .filter(|path| {
                    let path = path.as_str();
                    path.ends_with(token) && path[..path.len() - token.len()].ends_with('/')
                })
                .collect()
        }
    };
    match matches.as_slice() {
        [path] => Some((*path).clone()),
        _ => None,
    }
}

fn clean_reason(text: &str) -> String {
    let joined = text.replace('→', " ").replace("->", " ");
    let joined = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = joined
        .trim_matches(|c: char| c.is_whitespace() || "(),.:;-–".contains(c))
        .to_string();
    const MAX_CHARS: usize = 100;
    if cleaned.chars().count() > MAX_CHARS {
        let cut: String = cleaned.chars().take(MAX_CHARS).collect();
        format!("{}…", cut.trim_end())
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_never_splits_a_character() {
        let text = "first line\nsecond — dash\nthird";
        // Byte 19 is inside the em dash.
        let cut = truncate_to_lines(text, 19);
        assert_eq!(cut, "first line");
        assert_eq!(truncate_to_lines("short", 100), "short");
    }

    #[test]
    fn trailing_commas_are_forgiven_outside_strings() {
        let fixed = strip_trailing_commas("[{\"a\": \"x, ]\", \"b\": 1,}, ]");
        assert_eq!(fixed, "[{\"a\": \"x, ]\", \"b\": 1} ]");
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok());
    }

    #[test]
    fn an_excerpt_numbers_added_lines_and_counts_both_sides() {
        let patch =
            "--- a/x\n+++ b/x\n@@ -3,2 +3,3 @@\n-old\n+new one\n+new two\n@@ -9,0 +11 @@\n+late\n";
        let (excerpt, added, removed) = excerpt_of(patch);
        assert_eq!(
            excerpt,
            "-     |old\n+    3 |new one\n+    4 |new two\n+   11 |late\n"
        );
        assert_eq!((added, removed), (3, 1));
    }

    #[test]
    fn the_description_order_pins_files_first() {
        let known = vec![
            "docs/a.md".to_string(),
            "src/b.rs".to_string(),
            "src/c.rs".to_string(),
        ];
        let pinned = order_from_description(
            "**Review order:** `src/b.rs` (the change) → `a.md`.",
            &known,
        );
        assert_eq!(
            pinned.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["src/b.rs", "docs/a.md"]
        );
        assert_eq!(pinned[0].reason, "the change");
        let model = vec![
            Entry {
                path: "src/c.rs".into(),
                reason: "c".into(),
                notes: Vec::new(),
            },
            Entry {
                path: "src/b.rs".into(),
                reason: "b".into(),
                notes: Vec::new(),
            },
            Entry {
                path: "docs/a.md".into(),
                reason: "a".into(),
                notes: Vec::new(),
            },
        ];
        let ordered = with_pinned_first(model, &pinned);
        assert_eq!(
            ordered.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(),
            vec!["src/b.rs", "docs/a.md", "src/c.rs"]
        );
        assert_eq!(ordered[1].reason, "a");
    }
}
