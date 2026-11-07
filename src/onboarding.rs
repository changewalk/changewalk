//! The two questions asked on a first run: which host the reviews are on,
//! and which editor to read them in. Both are answered once and kept in
//! [`Config`]; `changewalk --onboard` asks again.

use crate::config::{Config, Ide};
use crate::forge::Forge;
use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Padding, Paragraph},
};
use std::{process::Command, time::Duration};

const CHOSEN: Color = Color::Cyan;
const DIM: Color = Color::DarkGray;
const HEADING: Color = Color::Green;
const READY: Color = Color::Green;
const MISSING: Color = Color::Yellow;

/// The saved configuration, asking for one first if this is a first run.
/// `force` asks again even when there is one saved.
pub fn configure(force: bool) -> Result<Config> {
    if !force && let Some(config) = Config::load() {
        return Ok(config);
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        // Piped or scripted, as `--open` from an editor is: carry on with
        // the defaults rather than block on a question nobody can see.
        if force {
            bail!("--onboard needs a terminal to ask in");
        }
        let config = Config::default();
        eprintln!(
            "changewalk has not been set up; assuming {} and {}. Run `changewalk --onboard` in a terminal to choose.",
            config.forge.name(),
            config.ide.name()
        );
        return Ok(config);
    }
    let mut terminal = ratatui::init();
    let answers = ask(&mut terminal);
    ratatui::restore();
    let Some((forge, ide)) = answers? else {
        bail!("setup cancelled");
    };
    let config = Config {
        forge,
        ide,
        app: Some(ide.default_app()),
    };
    let path = config.save()?;
    println!(
        "changewalk will look for {} {} and open them in {} ({}).",
        forge.name(),
        forge.plural_noun().to_lowercase(),
        ide.name(),
        config.app().display()
    );
    println!("Saved to {}.", path.display());
    Ok(config)
}

/// None when the user backed out.
fn ask(terminal: &mut ratatui::DefaultTerminal) -> Result<Option<(Forge, Ide)>> {
    let mut asked = Asked::default();
    loop {
        let shown = asked;
        terminal.draw(|frame| draw(frame, shown.step, shown.forge, shown.ide))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match asked.press(key.code, key.modifiers) {
            Press::Waiting => {}
            Press::Cancelled => return Ok(None),
            Press::Answered(answers) => return Ok(Some(answers)),
        }
    }
}

/// Which question is up and what is marked on each, so pressing a key is a
/// plain function of what came before it.
#[derive(Clone, Copy, Default)]
struct Asked {
    step: Step,
    forge: usize,
    ide: usize,
}

enum Press {
    Waiting,
    Cancelled,
    Answered((Forge, Ide)),
}

impl Asked {
    fn press(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Press {
        let count = match self.step {
            Step::Forge => Forge::ALL.len(),
            Step::Ide => Ide::ALL.len(),
        };
        let choice = match self.step {
            Step::Forge => &mut self.forge,
            Step::Ide => &mut self.ide,
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => *choice = choice.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => *choice = (*choice + 1).min(count - 1),
            KeyCode::Char(digit @ '1'..='9') => {
                let wanted = digit as usize - '1' as usize;
                if wanted < count {
                    *choice = wanted;
                }
            }
            KeyCode::Enter => match self.step {
                Step::Forge => self.step = Step::Ide,
                Step::Ide => {
                    return Press::Answered((Forge::ALL[self.forge], Ide::ALL[self.ide]));
                }
            },
            // Back a question on the second; nothing to go back to on the
            // first, so it does not quit.
            KeyCode::Left | KeyCode::Backspace => {
                if matches!(self.step, Step::Ide) {
                    self.step = Step::Forge;
                }
            }
            KeyCode::Char('q') | KeyCode::Esc => return Press::Cancelled,
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                return Press::Cancelled;
            }
            _ => {}
        }
        Press::Waiting
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Step {
    #[default]
    Forge,
    Ide,
}

fn draw(frame: &mut Frame, step: Step, forge: usize, ide: usize) {
    let (number, question) = match step {
        Step::Forge => (1, "Select review platform"),
        Step::Ide => (2, "What do you read diffs in?"),
    };
    // The panel is as wide as it is allowed and as tall as what it holds,
    // so the text can be wrapped by hand: a detail that runs on keeps its
    // indent, which `Paragraph`'s own wrapping would drop. The borders and
    // the padding inside them both take room the text cannot use.
    let width = frame.area().width.min(PANEL_WIDTH);
    let text_width =
        usize::from(width.saturating_sub(2 + 2 * PAD_X)).max(16);
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Step {number} of 2"),
            Style::default().fg(DIM),
        )),
        Line::from(Span::styled(
            question.to_string(),
            Style::default().fg(HEADING).bold(),
        )),
        Line::from(""),
    ];
    match step {
        Step::Forge => {
            for (index, option) in Forge::ALL.into_iter().enumerate() {
                lines.push(option_line(index == forge, index, option.name()));
                lines.extend(detail(&login_state(option), text_width, DIM));
                lines.extend(detail(
                    &format!("or set {}", token_variables(option)),
                    text_width,
                    DIM,
                ));
                lines.push(Line::from(""));
            }
        }
        Step::Ide => {
            for (index, option) in Ide::ALL.into_iter().enumerate() {
                lines.push(option_line(index == ide, index, option.name()));
                lines.extend(detail(option.description(), text_width, DIM));
                let app = option.default_app();
                lines.extend(if app.exists() {
                    detail(&format!("found at {}", app.display()), text_width, READY)
                } else {
                    detail(
                        "not found; set ZED_APP or --zed-app to its path",
                        text_width,
                        MISSING,
                    )
                });
                lines.push(Line::from(""));
            }
        }
    }
    // Two border rows plus a padding row above and below the text.
    let height = u16::try_from(lines.len() + 2 + 2 * usize::from(PAD_Y)).unwrap_or(u16::MAX);
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::vertical([Constraint::Length(height.min(frame.area().height))])
        .flex(Flex::Center)
        .areas(area);
    let mut footer = vec![
        hint("↑↓", "choose"),
        hint("⏎", if number == 1 { "next" } else { "finish" }),
    ];
    // Only the second question has somewhere to go back to.
    if number == 2 {
        footer.push(hint("←", "previous"));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .padding(Padding::symmetric(PAD_X, PAD_Y))
        .title(Line::from(vec![
            Span::raw(" Welcome to "),
            Span::styled("changewalk", Style::default().bold()),
            Span::raw(" "),
        ]))
        .title_bottom(Line::from(footer));
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// How wide the panel is allowed to be.
const PANEL_WIDTH: u16 = 78;

/// Padding inside the border, so the text does not sit against it.
const PAD_X: u16 = 2;
const PAD_Y: u16 = 1;

fn option_line(selected: bool, index: usize, name: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            if selected { " ▶ " } else { "   " },
            Style::default().fg(CHOSEN),
        ),
        Span::styled(format!("{} ", index + 1), Style::default().fg(DIM)),
        Span::styled(
            name.to_string(),
            if selected {
                Style::default().fg(CHOSEN).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ])
}

/// An option's explanation, indented under it and wrapped by hand so a
/// line that runs on stays indented too.
fn detail(text: &str, width: usize, colour: Color) -> Vec<Line<'static>> {
    const INDENT: &str = "     ";
    let room = width.saturating_sub(INDENT.len()).max(16);
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > room {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
        .into_iter()
        .map(|line| {
            Line::from(Span::styled(
                format!("{INDENT}{line}"),
                Style::default().fg(colour),
            ))
        })
        .collect()
}

fn hint(key: &str, action: &str) -> Span<'static> {
    Span::styled(format!(" {key} {action} "), Style::default().fg(DIM))
}

/// Whether the host's command-line tool is installed, said in a few words
/// so the choice is an informed one.
fn login_state(forge: Forge) -> String {
    let cli = forge.cli();
    if Command::new("which")
        .arg(cli)
        .output()
        .is_ok_and(|output| output.status.success())
    {
        format!("`{cli}` is installed — log in with `{cli} auth login`")
    } else {
        format!("`{cli}` is not installed — `brew install {cli}`, then `{cli} auth login`")
    }
}

fn token_variables(forge: Forge) -> String {
    match forge {
        Forge::Gitlab => "GITLAB_TOKEN in the environment".into(),
        Forge::Github => "GITHUB_TOKEN or GH_TOKEN in the environment".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn screen(step: Step, forge: usize, ide: usize) -> String {
        screen_at(step, forge, ide, 90, 24)
    }

    fn screen_at(step: Step, forge: usize, ide: usize, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, step, forge, ide))
            .unwrap();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(keys: &[KeyCode]) -> Option<(Forge, Ide)> {
        let mut asked = Asked::default();
        for &key in keys {
            match asked.press(key, KeyModifiers::NONE) {
                Press::Waiting => {}
                Press::Cancelled => return None,
                Press::Answered(answers) => return Some(answers),
            }
        }
        panic!("the questions were neither answered nor cancelled");
    }

    /// The two questions, answered the ways they can be answered.
    #[test]
    fn keys_walk_the_two_questions_and_back_out_of_them() {
        use KeyCode::{Backspace, Char, Down, Enter, Esc, Left, Up};
        // Straight through, taking what is already marked.
        assert_eq!(press(&[Enter, Enter]), Some((Forge::Gitlab, Ide::Zed)));
        // Down to GitHub, then on.
        assert_eq!(
            press(&[Down, Enter, Enter]),
            Some((Forge::Github, Ide::Zed))
        );
        assert_eq!(
            press(&[Char('j'), Enter, Enter]),
            Some((Forge::Github, Ide::Zed))
        );
        // By number.
        assert_eq!(
            press(&[Char('2'), Enter, Enter]),
            Some((Forge::Github, Ide::Zed))
        );
        // Marking cannot run off either end.
        assert_eq!(
            press(&[Up, Up, Enter, Enter]),
            Some((Forge::Gitlab, Ide::Zed))
        );
        assert_eq!(
            press(&[Down, Down, Down, Enter, Enter]),
            Some((Forge::Github, Ide::Zed))
        );
        // A digit with nothing behind it changes nothing.
        assert_eq!(
            press(&[Char('9'), Enter, Enter]),
            Some((Forge::Gitlab, Ide::Zed))
        );
        // Back from the second question, and a different answer.
        assert_eq!(
            press(&[Down, Enter, Left, Up, Enter, Enter]),
            Some((Forge::Gitlab, Ide::Zed))
        );
        assert_eq!(
            press(&[Enter, Backspace, Down, Enter, Enter]),
            Some((Forge::Github, Ide::Zed))
        );
        // Left on the first question does nothing: it neither quits nor
        // answers, so the questions can still be answered after it.
        assert_eq!(
            press(&[Left, Backspace, Enter, Enter]),
            Some((Forge::Gitlab, Ide::Zed))
        );

        // Backing out, which Left on the first question must not do.
        assert_eq!(press(&[Esc]), None);
        assert_eq!(press(&[Char('q')]), None);
        assert_eq!(press(&[Enter, Char('q')]), None);
    }

    /// A detail too long for the panel wraps under its own indent rather
    /// than back to the border, at any width.
    #[test]
    fn an_explanation_that_runs_on_stays_indented() {
        for width in [48, 64, 90, 120] {
            let screen = screen_at(Step::Forge, 0, 0, width, 30);
            for row in screen.lines() {
                let Some((_, inside)) = row.split_once('│') else {
                    continue;
                };
                let inside = inside.trim_end_matches(['│', ' ']);
                assert!(
                    inside.is_empty() || inside.starts_with(' '),
                    "a line reached the border at width {width}: {row:?}"
                );
            }
        }
    }

    #[test]
    fn the_first_step_offers_both_hosts_and_the_second_the_editor() {
        let hosts = screen(Step::Forge, 1, 0);
        assert!(hosts.contains("Step 1 of 2"), "{hosts}");
        assert!(hosts.contains("Select review platform"), "{hosts}");
        assert!(hosts.contains("1 GitLab"), "{hosts}");
        // The second host is the one marked, since it was asked for.
        assert!(hosts.contains("▶ 2 GitHub"), "{hosts}");
        assert!(hosts.contains("GITHUB_TOKEN"), "{hosts}");
        // The first question has nowhere to go back to, and the quit hint
        // is gone.
        assert!(!hosts.contains("previous"), "{hosts}");
        assert!(!hosts.contains("cancel"), "{hosts}");
        assert!(!hosts.contains("q cancel"), "{hosts}");

        let editors = screen(Step::Ide, 1, 0);
        assert!(editors.contains("Step 2 of 2"), "{editors}");
        assert!(editors.contains("▶ 1 Zed"), "{editors}");
        assert!(editors.contains("review order"), "{editors}");
        // Only the second question offers going back, and reads "previous".
        assert!(editors.contains("← previous"), "{editors}");
        assert!(!editors.contains("cancel"), "{editors}");
    }
}
