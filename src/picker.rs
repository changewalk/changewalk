//! The terminal list of merge requests.

use crate::gitlab::{self, LastOpened, MergeRequest, Project, Role};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::Duration,
};

/// One step of a refresh, each carrying the project's whole list with more
/// of it filled in, so the screen can show what is known so far.
enum FetchStage {
    /// Titles, authors, and bases; cached details carried over where the
    /// merge request has not changed.
    Listed(Vec<MergeRequest>),
    /// Approvals and comment and line counts from GitLab.
    Detailed(Vec<MergeRequest>),
    /// Line counts retaken from the clone after a fetch.
    Counted(Vec<MergeRequest>),
    Done,
}

type FetchResult = (Project, Result<FetchStage>);

/// Opens a merge request in Zed and says what happened, or why not.
type Launcher = dyn Fn(&MergeRequest) -> Result<String> + Send + Sync;

const NUMBER: Color = Color::Cyan;
const AUTHOR: Color = Color::Magenta;
const DRAFT: Color = Color::DarkGray;
const STACKED: Color = Color::Yellow;
const DIM: Color = Color::DarkGray;
const HEADING: Color = Color::Green;
const CODE: Color = Color::LightBlue;
const OPENED: Color = Color::LightGreen;
const APPROVED: Color = Color::Green;
const ADDED: Color = Color::Green;
const REMOVED: Color = Color::Red;

/// Marks the merge request most recently opened in Zed.
const OPENED_MARK: &str = "●";

struct Picker {
    items: Vec<MergeRequest>,
    selected: usize,
    description_scroll: u16,
    /// How far the description can scroll before its last line reaches the
    /// top, as measured at the last draw; the wrapped height depends on the
    /// terminal width, which only the draw knows.
    description_max_scroll: std::cell::Cell<u16>,
    title: String,
    status: String,
    pending_fetches: usize,
    default_branches: HashMap<PathBuf, String>,
    last_opened: Option<LastOpened>,
    projects: Vec<Project>,
    role: Role,
    /// Fresh lists arrive here while a refresh is running.
    fetches: Option<Receiver<FetchResult>>,
    /// The launch in progress, which runs on its own thread because the
    /// fetch and checkout take a moment.
    launching: Option<Receiver<Result<String>>>,
}

/// Shows the list until the user quits. Enter hands the selected merge
/// request to `launcher` and keeps the list open; `r` fetches fresh lists
/// for `projects`. With nothing cached the first fetch runs at once, since
/// an empty list would say nothing.
pub fn run(
    cached: Vec<MergeRequest>,
    projects: Vec<Project>,
    role: Role,
    default_branches: HashMap<PathBuf, String>,
    title: String,
    launcher: impl Fn(&MergeRequest) -> Result<String> + Send + Sync + 'static,
) -> Result<()> {
    let nothing_cached = cached.is_empty();
    let mut picker = Picker {
        items: gitlab::order_as_stacks(cached, &default_branches),
        selected: 0,
        description_scroll: 0,
        description_max_scroll: std::cell::Cell::new(0),
        title,
        status: "press r to refresh".into(),
        pending_fetches: 0,
        default_branches,
        last_opened: gitlab::last_opened(),
        projects,
        role,
        fetches: None,
        launching: None,
    };
    if nothing_cached {
        picker.refresh();
    }
    let launcher: Arc<Launcher> = Arc::new(launcher);
    let mut terminal = ratatui::init();
    let outcome = picker.event_loop(&mut terminal, &launcher);
    ratatui::restore();
    outcome
}

impl Picker {
    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        launcher: &Arc<Launcher>,
    ) -> Result<()> {
        loop {
            self.receive();
            self.receive_launch();
            terminal.draw(|frame| self.draw(frame))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => self.select(self.selected + 1),
                KeyCode::Home => self.select(0),
                KeyCode::End | KeyCode::Char('G') => self.select(usize::MAX),
                KeyCode::PageUp | KeyCode::Char('u') => {
                    self.description_scroll = self.description_scroll.saturating_sub(10)
                }
                KeyCode::PageDown | KeyCode::Char('d') => {
                    self.description_scroll = self
                        .description_scroll
                        .saturating_add(10)
                        .min(self.description_max_scroll.get())
                }
                KeyCode::Enter => self.launch(launcher),
                KeyCode::Char('r') => self.refresh(),
                KeyCode::Char('g') | KeyCode::Char('o') => {
                    if let Some(merge_request) = self.items.get(self.selected) {
                        Command::new("open")
                            .arg(&merge_request.web_url)
                            .status()
                            .ok();
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    /// Opens the selected merge request in Zed on a thread, so the list
    /// stays live, and remembers it as the last one opened.
    fn launch(&mut self, launcher: &Arc<Launcher>) {
        if self.launching.is_some() {
            self.status = "still opening the last one; a fetch may be waiting on git".into();
            return;
        }
        let Some(merge_request) = self.items.get(self.selected).cloned() else {
            return;
        };
        self.status = format!("opening !{} in Zed…", merge_request.iid);
        self.last_opened = Some(gitlab::remember_opened(&merge_request));
        let (sender, receiver) = mpsc::channel();
        let launcher = launcher.clone();
        thread::spawn(move || {
            sender.send(launcher(&merge_request)).ok();
        });
        self.launching = Some(receiver);
    }

    /// Asks GitLab for every project's list on its own thread, unless a
    /// refresh is already running, and brings the clone's remote branches
    /// up to date at the same time, so the diffs Zed draws are against
    /// current bases.
    fn refresh(&mut self) {
        if self.pending_fetches > 0 {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        for project in self.projects.clone() {
            let sender = sender.clone();
            let role = self.role;
            let cached: Vec<MergeRequest> = self
                .items
                .iter()
                .filter(|mr| mr.repo == project.repo)
                .cloned()
                .collect();
            thread::spawn(move || {
                let send = |stage: Result<FetchStage>| sender.send((project.clone(), stage)).ok();
                let mut list = match gitlab::fetch_lists(&project, role, &cached) {
                    Ok(list) => list,
                    Err(error) => {
                        send(Err(error));
                        send(Ok(FetchStage::Done));
                        return;
                    }
                };
                send(Ok(FetchStage::Listed(list.clone())));
                if let Err(error) = gitlab::fill_details(&project, &mut list) {
                    send(Err(error));
                }
                send(Ok(FetchStage::Detailed(list.clone())));
                // A fetch from a moment ago, as a launch does, is good enough.
                if !gitlab::git_fetched_recently(&project) {
                    match gitlab::git(&project.repo, &["fetch", "--prune", "origin"]) {
                        Ok(_) => gitlab::mark_git_fetched(&project),
                        Err(error) => {
                            send(Err(error));
                        }
                    }
                }
                // GitLab's counts are from the merge base, which shows a
                // squash-merged parent's work again; count the diff Zed will
                // show, but only where the branch has moved since last time.
                for merge_request in &mut list {
                    let tip = gitlab::git(
                        &project.repo,
                        &[
                            "rev-parse",
                            &format!("origin/{}", merge_request.source_branch),
                        ],
                    )
                    .ok();
                    if tip.is_some() && tip == merge_request.counted_sha {
                        continue;
                    }
                    if let Some((added, removed)) = crate::launch::local_diff_size(merge_request) {
                        merge_request.additions = added;
                        merge_request.deletions = removed;
                        merge_request.counted_sha = tip;
                    }
                }
                gitlab::write_cache(&project, &list);
                send(Ok(FetchStage::Counted(list.clone())));
                send(Ok(FetchStage::Done));
                // Last, and after the list is reported done: the model is
                // asked for the reading order of the top few, so Zed can
                // show it at once when one is opened.
                crate::warm::warm_order_cache(&project, &list, 3);
            });
        }
        self.pending_fetches = self.projects.len();
        self.fetches = Some(receiver);
        self.status = "refreshing…".into();
    }

    fn receive_launch(&mut self) {
        let Some(receiver) = &self.launching else {
            return;
        };
        let opened = match receiver.try_recv() {
            Ok(Ok(message)) => {
                self.status = message;
                true
            }
            Ok(Err(error)) => {
                self.status = format!("could not open: {error:#}");
                false
            }
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.status = "could not open: launch stopped".into();
                false
            }
        };
        self.launching = None;
        // The launch fetched the clone, so the list's counts and bases can
        // be brought in line with what Zed is now showing.
        if opened {
            let message = self.status.clone();
            self.refresh();
            self.status = format!("{message} · refreshing…");
        }
    }

    fn is_last_opened(&self, merge_request: &MergeRequest) -> bool {
        self.last_opened
            .as_ref()
            .is_some_and(|opened| opened.web_url == merge_request.web_url)
    }

    fn select(&mut self, index: usize) {
        let last = self.items.len().saturating_sub(1);
        let index = index.min(last);
        if index != self.selected {
            self.selected = index;
            self.description_scroll = 0;
        }
    }

    fn receive(&mut self) {
        let Some(receiver) = &self.fetches else {
            return;
        };
        loop {
            match receiver.try_recv() {
                Ok((project, Ok(stage))) => {
                    let fresh = match stage {
                        FetchStage::Listed(list)
                        | FetchStage::Detailed(list)
                        | FetchStage::Counted(list) => list,
                        FetchStage::Done => {
                            self.pending_fetches = self.pending_fetches.saturating_sub(1);
                            if self.pending_fetches == 0 && !self.status.starts_with("could not") {
                                self.status = format!(
                                    "refreshed {}",
                                    local_time(&chrono::Local::now().to_rfc3339())
                                );
                            }
                            continue;
                        }
                    };
                    let selected_iid = self.items.get(self.selected).map(|mr| mr.iid);
                    let mut items: Vec<MergeRequest> = self
                        .items
                        .drain(..)
                        .filter(|mr| mr.repo != project.repo)
                        .collect();
                    items.extend(fresh);
                    self.items = gitlab::order_as_stacks(items, &self.default_branches);
                    self.selected = selected_iid
                        .and_then(|iid| self.items.iter().position(|mr| mr.iid == iid))
                        .unwrap_or(0);
                    self.status = "refreshing…".into();
                }
                Ok((_, Err(error))) => {
                    self.status = format!("could not refresh: {error:#}");
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.pending_fetches = 0;
                    self.fetches = None;
                    return;
                }
            }
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let [list, description] =
            Layout::vertical([Constraint::Percentage(55), Constraint::Min(5)]).areas(frame.area());
        self.draw_list(frame, list);
        self.draw_description(frame, description);
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(DIM))
            .title(Line::from(vec![
                Span::raw(" Merge requests to review · "),
                Span::styled(self.title.clone(), Style::default().bold()),
                Span::raw(" "),
            ]));

        // The parent of a stacked merge request, for the branch column.
        let parent_by_source: HashMap<(&PathBuf, &str), u64> = self
            .items
            .iter()
            .map(|mr| ((&mr.repo, mr.source_branch.as_str()), mr.iid))
            .collect();

        let rows = self.items.iter().enumerate().map(|(index, mr)| {
            let selected = index == self.selected;
            let opened = self.is_last_opened(mr);
            let guide = if mr.depth > 0 {
                format!("{}└", "  ".repeat(mr.depth - 1))
            } else {
                String::new()
            };
            let number = Line::from(vec![
                Span::styled(
                    if selected { "▶ " } else { "  " },
                    Style::default().fg(NUMBER),
                ),
                Span::styled(guide, Style::default().fg(DIM)),
                Span::styled(
                    format!("!{}", mr.iid),
                    Style::default().fg(if opened { OPENED } else { NUMBER }),
                ),
                Span::styled(
                    if opened {
                        format!(" {OPENED_MARK}")
                    } else {
                        String::new()
                    },
                    Style::default().fg(OPENED),
                ),
            ]);
            let mut title = styled_title(&mr.title);
            if mr.draft && !mr.title.to_ascii_lowercase().starts_with("draft") {
                title.spans.insert(
                    0,
                    Span::styled("Draft · ", Style::default().fg(DRAFT).italic()),
                );
            }
            let author = if mr.mine {
                Span::styled("you", Style::default().fg(NUMBER))
            } else {
                Span::styled(mr.author_name().to_string(), Style::default().fg(AUTHOR))
            };
            let target = if mr.stacked {
                match parent_by_source.get(&(&mr.repo, mr.target_branch.as_str())) {
                    Some(parent) => {
                        Span::styled(format!("↑ !{parent}"), Style::default().fg(STACKED))
                    }
                    None => Span::styled(
                        format!("↑ {}", mr.target_branch),
                        Style::default().fg(STACKED),
                    ),
                }
            } else {
                Span::styled(format!("→ {}", mr.target_branch), Style::default().fg(DIM))
            };
            let updated = Span::styled(local_time(&mr.updated_at), Style::default().fg(DIM));
            let approved = Span::styled(approval_summary(mr), Style::default().fg(APPROVED));
            let comments = comment_summary(mr);
            let size = Line::from(vec![
                Span::styled(format!("+{}", mr.additions), Style::default().fg(ADDED)),
                Span::raw(" "),
                Span::styled(format!("−{}", mr.deletions), Style::default().fg(REMOVED)),
            ]);
            let row = Row::new(vec![
                Cell::from(number),
                Cell::from(title),
                Cell::from(author),
                Cell::from(target),
                Cell::from(size),
                Cell::from(comments),
                Cell::from(approved),
                Cell::from(updated),
            ]);
            if selected {
                row.style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED))
            } else {
                row
            }
        });

        let widths = [
            Constraint::Length(14),
            Constraint::Fill(3),
            Constraint::Length(20),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(12),
        ];
        let header = Row::new([
            "", "Title", "Author", "Base", "Diff", "Comments", "Approved", "Updated",
        ])
        .style(Style::default().fg(DIM).add_modifier(Modifier::BOLD));
        let table = Table::new(rows, widths)
            .header(header)
            .column_spacing(2)
            .block(block);
        let mut state = TableState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn draw_description(&self, frame: &mut Frame, area: Rect) {
        let mut footer = vec![
            key_hint("↑↓", "move"),
            key_hint("⏎", "open in Zed"),
            key_hint("g", "GitLab"),
            key_hint("r", "refresh"),
            key_hint("u/d", "scroll"),
            key_hint("G", "last"),
            key_hint("q", "quit"),
        ];
        if let Some(opened) = &self.last_opened
            && let Some(merge_request) = self.items.iter().find(|mr| mr.web_url == opened.web_url)
        {
            footer.push(Span::styled(
                format!(
                    " {OPENED_MARK} !{} opened {} ",
                    merge_request.iid,
                    local_time(&opened.at)
                ),
                Style::default().fg(OPENED),
            ));
        }
        footer.push(Span::styled(
            format!(" {} ", self.status),
            Style::default().fg(DIM).italic(),
        ));
        let footer = Line::from(footer);
        let Some(merge_request) = self.items.get(self.selected) else {
            let text = if self.pending_fetches == 0 {
                " No open merge requests."
            } else {
                " Loading…"
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(DIM))
                .title_bottom(footer);
            frame.render_widget(Paragraph::new(text).block(block), area);
            return;
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(DIM))
            .title(Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    format!("!{}", merge_request.iid),
                    Style::default().fg(NUMBER),
                ),
                Span::raw(" · "),
                Span::styled(merge_request.title.clone(), Style::default().bold()),
                Span::raw(" "),
            ]))
            .title_bottom(footer);
        let text = merge_request
            .description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("(no description)");
        let mut lines = vec![Line::from(Span::styled(
            format!(" {}", merge_request.web_url),
            Style::default().fg(DIM),
        ))];
        if !merge_request.approved_by.is_empty() {
            lines.push(Line::from(Span::styled(
                format!(" ✓ Approved by {}", merge_request.approved_by.join(", ")),
                Style::default().fg(APPROVED),
            )));
        }
        lines.push(Line::from(""));
        lines.extend(render_markdown(text));
        let inner = block.inner(area);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let max_scroll = paragraph
            .line_count(inner.width)
            .saturating_sub(usize::from(inner.height));
        let max_scroll = u16::try_from(max_scroll).unwrap_or(u16::MAX);
        self.description_max_scroll.set(max_scroll);
        frame.render_widget(
            paragraph
                .scroll((self.description_scroll.min(max_scroll), 0))
                .block(block),
            area,
        );
    }
}

/// `12`, with `· 3 you` when some of the comments are the user's own;
/// nothing when there are none.
fn comment_summary(merge_request: &MergeRequest) -> Line<'static> {
    if merge_request.comment_count == 0 {
        return Line::from("");
    }
    let mut spans = vec![Span::styled(
        merge_request.comment_count.to_string(),
        Style::default().fg(DIM),
    )];
    if merge_request.my_comment_count > 0 {
        spans.push(Span::styled(
            format!(" · {} you", merge_request.my_comment_count),
            Style::default().fg(NUMBER),
        ));
    }
    Line::from(spans)
}

/// `✓ you`, `✓ you +2`, or `✓ 3`, for the approved column; nothing when no
/// one has approved.
fn approval_summary(merge_request: &MergeRequest) -> String {
    let count = merge_request.approved_by.len();
    if count == 0 {
        String::new()
    } else if merge_request.user_has_approved {
        match count - 1 {
            0 => "✓ you".to_string(),
            others => format!("✓ you +{others}"),
        }
    } else {
        format!("✓ {count}")
    }
}

fn key_hint(key: &str, action: &str) -> Span<'static> {
    Span::styled(format!(" {key} {action} "), Style::default().fg(DIM))
}

/// An RFC 3339 timestamp as `19 Oct 11:32` in local time, or the date alone
/// when the time cannot be read.
fn local_time(timestamp: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(timestamp) {
        Ok(time) => time
            .with_timezone(&chrono::Local)
            .format("%-d %b %H:%M")
            .to_string(),
        Err(_) => short_date(timestamp.get(..10).unwrap_or(timestamp)),
    }
}

/// `2026-10-19` as `19 Oct`.
fn short_date(day: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = day.splitn(3, '-');
    let (Some(_year), Some(month), Some(date)) = (parts.next(), parts.next(), parts.next()) else {
        return day.to_string();
    };
    let month = month
        .parse::<usize>()
        .ok()
        .and_then(|month| MONTHS.get(month.wrapping_sub(1)))
        .copied()
        .unwrap_or(month);
    format!("{} {month}", date.trim_start_matches('0'))
}

/// The conventional-commit prefix dimmed, so the subject stands out.
fn styled_title(title: &str) -> Line<'static> {
    match title.split_once(": ") {
        Some((kind, subject)) if kind.len() <= 24 && !kind.contains(' ') => Line::from(vec![
            Span::styled(format!("{kind}: "), Style::default().fg(DIM)),
            Span::raw(subject.to_string()),
        ]),
        _ => Line::from(title.to_string()),
    }
}

/// A light reading of markdown for the terminal: headings, bullets, bold,
/// inline code, and links shown as their text. Runs of blank lines collapse.
fn render_markdown(text: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut previous_blank = true;
    let mut in_code_block = false;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            previous_blank = false;
            continue;
        }
        if in_code_block {
            lines.push(Line::from(Span::styled(
                format!("   {line}"),
                Style::default().fg(CODE),
            )));
            previous_blank = false;
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            if !previous_blank {
                lines.push(Line::from(""));
            }
            previous_blank = true;
            continue;
        }
        previous_blank = false;
        if trimmed.starts_with("<details") || trimmed.starts_with("</details") {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix('#') {
            let heading = rest.trim_start_matches('#').trim();
            lines.push(Line::from(Span::styled(
                format!(" {heading}"),
                Style::default().fg(HEADING).bold(),
            )));
            continue;
        }
        if let Some(summary) = trimmed
            .strip_prefix("<summary>")
            .and_then(|rest| rest.strip_suffix("</summary>"))
        {
            lines.push(Line::from(Span::styled(
                format!(" {summary}"),
                Style::default().fg(HEADING).bold(),
            )));
            continue;
        }
        let indent = line.len() - trimmed.len();
        let (prefix, body) = if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            (format!("{}  • ", " ".repeat(indent)), rest)
        } else {
            (format!("{} ", " ".repeat(indent)), trimmed)
        };
        let mut spans = vec![Span::raw(prefix)];
        spans.extend(inline_spans(body));
        lines.push(Line::from(spans));
    }
    lines
}

/// Inline markdown: `code`, **bold**, and [text](url).
fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let next_code = rest.find('`');
        let next_bold = rest.find("**");
        let next_link = rest.find('[');
        let next = [next_code, next_bold, next_link]
            .into_iter()
            .flatten()
            .min();
        let Some(start) = next else {
            spans.push(Span::raw(rest.to_string()));
            break;
        };
        if start > 0 {
            spans.push(Span::raw(rest[..start].to_string()));
        }
        let tail = &rest[start..];
        if let Some(inner) = tail.strip_prefix('`') {
            if let Some(end) = inner.find('`') {
                spans.push(Span::styled(
                    inner[..end].to_string(),
                    Style::default().fg(CODE),
                ));
                rest = &inner[end + 1..];
                continue;
            }
        } else if let Some(inner) = tail.strip_prefix("**") {
            if let Some(end) = inner.find("**") {
                spans.push(Span::styled(
                    inner[..end].to_string(),
                    Style::default().bold(),
                ));
                rest = &inner[end + 2..];
                continue;
            }
        } else if let Some(inner) = tail.strip_prefix('[')
            && let Some((label, after)) = inner.split_once("](")
            && let Some(end) = after.find(')')
        {
            spans.push(Span::styled(
                label.to_string(),
                Style::default().underlined(),
            ));
            rest = &after[end + 1..];
            continue;
        }
        // Not markup after all: emit the character and move on.
        let char_len = tail.chars().next().map_or(1, char::len_utf8);
        spans.push(Span::raw(tail[..char_len].to_string()));
        rest = &tail[char_len..];
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_shorten_and_titles_dim_their_kind() {
        assert_eq!(short_date("2026-10-19"), "19 Oct");
        assert_eq!(short_date("2026-01-05"), "5 Jan");
        assert_eq!(short_date("odd"), "odd");
        assert!(local_time("2026-10-19T10:00:00.000Z").starts_with("19 Oct "));
        assert_eq!(
            local_time("2026-10-19T10:00:00.000Z").len(),
            "19 Oct 11:00".len()
        );
        assert_eq!(local_time("2026-10-19"), "19 Oct");
        let line = styled_title("feat(kernel): a reply is a result");
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[1].content, "a reply is a result");
        assert_eq!(styled_title("Plain title").spans.len(), 1);
    }

    #[test]
    fn frame_lays_out_columns_and_description() {
        use ratatui::{Terminal, backend::TestBackend};
        let mr = |iid: u64, title: &str, source: &str, target: &str, depth: usize| MergeRequest {
            iid,
            title: title.into(),
            description: Some(
                "## Why\n\nA `reply` becomes a **stored** result.\n- one\n- two".into(),
            ),
            author: Some(crate::gitlab::Author {
                name: "Tim Barnes".into(),
                username: "tim".into(),
            }),
            source_branch: source.into(),
            target_branch: target.into(),
            updated_at: "2026-10-19T10:00:00Z".into(),
            web_url: "https://gitlab.com/x/-/merge_requests/1628".into(),
            repo: PathBuf::from("/r"),
            depth,
            stacked: target != "main",
            approved_by: if depth == 0 {
                vec!["Tim Barnes".into(), "Jack Walters".into()]
            } else {
                Vec::new()
            },
            user_has_approved: depth == 0,
            additions: 193,
            deletions: 6,
            comment_count: 11,
            my_comment_count: if depth == 0 { 3 } else { 0 },
            diff_refs: None,
            draft: depth == 1,
            mine: depth == 1,
            counted_sha: None,
            details_current: false,
        };
        let picker = Picker {
            items: vec![
                mr(
                    1628,
                    "feat(kernel): a decision-scoped tool reply is a tool result",
                    "a",
                    "main",
                    0,
                ),
                mr(
                    1629,
                    "feat(kernel): a cited tool result is read as a number",
                    "b",
                    "a",
                    1,
                ),
            ],
            selected: 0,
            description_scroll: 0,
            description_max_scroll: std::cell::Cell::new(0),
            title: "group/project".into(),
            status: "up to date".into(),
            pending_fetches: 0,
            default_branches: HashMap::new(),
            projects: Vec::new(),
            role: Role::Reviewer,
            fetches: None,
            last_opened: Some(LastOpened {
                web_url: "https://gitlab.com/x/-/merge_requests/1628".into(),
                at: "2026-10-19T11:32:00+01:00".into(),
            }),
            launching: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(130, 20)).unwrap();
        terminal.draw(|frame| picker.draw(frame)).unwrap();
        let frame: Vec<String> = (0..20)
            .map(|y| {
                (0..130)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        for line in &frame {
            eprintln!("{line}");
        }
        let text = frame.join("\n");
        assert!(text.contains("▶ !1628 ●"));
        assert!(text.contains("└!1629"));
        assert!(text.contains("● !1628 opened 19 Oct"));
        assert!(text.contains("✓ you +1"));
        assert!(text.contains("+193 −6"));
        assert!(text.contains("11 · 3 you"));
        assert!(text.contains("Draft · feat(kernel"));
        assert!(text.contains("Draft · feat(kernel): a ci  you"));
        assert!(text.contains("✓ Approved by Tim Barnes, Jack Walters"));
        assert!(text.contains("↑ !1628"));
        assert!(text.contains("19 Oct 1"));
        assert!(text.contains(" Why"));
        assert!(text.contains("• one"));
    }

    #[test]
    fn markdown_renders_headings_bullets_and_inline_markup() {
        let lines = render_markdown(
            "## Why\n\n\n- see `code` and **bold** [here](http://x)\n```\nraw\n```\n",
        );
        let texts: Vec<String> = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect()
            })
            .collect();
        assert_eq!(
            texts,
            vec![" Why", "", "  • see code and bold here", "   raw"]
        );
    }
}
