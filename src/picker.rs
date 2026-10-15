//! The terminal list of merge requests.

use crate::gitlab::{self, MergeRequest, Project};
use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Command,
    sync::mpsc::{Receiver, TryRecvError},
    time::Duration,
};

type FetchResult = (Project, Result<Vec<MergeRequest>>);

struct Picker {
    items: Vec<MergeRequest>,
    selected: usize,
    title: String,
    status: String,
    pending_fetches: usize,
    default_branches: HashMap<PathBuf, String>,
}

/// Shows the list until the user picks a merge request or quits. Fresh lists
/// arriving on `receiver` replace the cached ones for their project.
pub fn run(
    cached: Vec<MergeRequest>,
    receiver: Receiver<FetchResult>,
    pending_fetches: usize,
    default_branches: HashMap<PathBuf, String>,
    title: String,
) -> Result<Option<MergeRequest>> {
    let mut picker = Picker {
        items: gitlab::order_as_stacks(cached, &default_branches),
        selected: 0,
        title,
        status: if pending_fetches > 0 {
            "refreshing…".into()
        } else {
            String::new()
        },
        pending_fetches,
        default_branches,
    };
    let mut terminal = ratatui::init();
    let outcome = picker.event_loop(&mut terminal, &receiver);
    ratatui::restore();
    outcome
}

impl Picker {
    fn event_loop(
        &mut self,
        terminal: &mut ratatui::DefaultTerminal,
        receiver: &Receiver<FetchResult>,
    ) -> Result<Option<MergeRequest>> {
        loop {
            self.receive(receiver);
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
                KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(self.items.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    if let Some(merge_request) = self.items.get(self.selected) {
                        return Ok(Some(merge_request.clone()));
                    }
                }
                KeyCode::Char('o') => {
                    if let Some(merge_request) = self.items.get(self.selected) {
                        Command::new("open").arg(&merge_request.web_url).status().ok();
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }

    fn receive(&mut self, receiver: &Receiver<FetchResult>) {
        loop {
            match receiver.try_recv() {
                Ok((project, Ok(fresh))) => {
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
                    self.pending_fetches = self.pending_fetches.saturating_sub(1);
                    self.status = if self.pending_fetches > 0 {
                        "refreshing…".into()
                    } else {
                        "up to date".into()
                    };
                }
                Ok((_, Err(error))) => {
                    self.pending_fetches = self.pending_fetches.saturating_sub(1);
                    self.status = format!("{error:#}");
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return,
            }
        }
    }

    fn draw(&self, frame: &mut Frame) {
        let [header, list, description, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Percentage(50),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!(" Merge requests to review in {}", self.title),
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    " ↑↓ move   enter open in Zed   o open in browser   q quit   ⇡ stacked on another branch",
                    Style::default().fg(Color::DarkGray),
                )),
            ]),
            header,
        );

        self.draw_list(frame, list);

        if let Some(merge_request) = self.items.get(self.selected) {
            let mut lines = vec![
                Line::from(Span::styled(
                    format!(" {}", merge_request.title),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    format!(" {}", merge_request.web_url),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(""),
            ];
            let text = merge_request.description.as_deref().unwrap_or("(no description)");
            lines.extend(text.lines().map(|line| Line::from(format!(" {line}"))));
            frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), description);
        } else if self.pending_fetches == 0 {
            frame.render_widget(Paragraph::new(" No open merge requests."), description);
        }

        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}", self.status),
                Style::default().fg(Color::DarkGray),
            )),
            footer,
        );
    }

    fn draw_list(&self, frame: &mut Frame, area: Rect) {
        let width = area.width as usize;
        let items: Vec<ListItem> = self
            .items
            .iter()
            .map(|merge_request| {
                let indent = "  ".repeat(merge_request.depth)
                    + if merge_request.depth > 0 { "└ " } else { "" };
                let prefix = format!("{indent}!{}", merge_request.iid);
                let target = if merge_request.stacked {
                    format!("⇡ {}", merge_request.target_branch)
                } else {
                    format!("→ {}", merge_request.target_branch)
                };
                let suffix = format!(
                    "  {} · {} {} · {} ",
                    merge_request.author_name(),
                    merge_request.source_branch,
                    target,
                    merge_request.updated_day()
                );
                let available = width.saturating_sub(1 + suffix.chars().count());
                let mut body = format!(" {prefix:<9} {}", merge_request.title);
                body = body.chars().take(available).collect();
                let padding = available.saturating_sub(body.chars().count());
                let line = format!("{body}{}{suffix}", " ".repeat(padding));
                ListItem::new(Line::from(line))
            })
            .collect();
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(
            List::new(items).highlight_style(Style::default().bg(Color::Cyan).fg(Color::Black)),
            area,
            &mut state,
        );
    }
}
