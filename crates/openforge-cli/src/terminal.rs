use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{self, ClearType},
};
use std::io::{self, IsTerminal, Write};

pub const COMMANDS: &[(&str, &str)] = &[
    ("/help", "show all commands"),
    ("/init", "create AGENTS.md without overwriting it"),
    ("/review", "review tracked changes with a read-only run"),
    ("/status", "session, workspace, policy, goal and last run"),
    ("/model", "inspect available models and active gateway"),
    ("/permissions", "show or select the active policy"),
    ("/approvals", "alias for /permissions"),
    ("/plan", "enter planning mode; optional inline objective"),
    ("/mode", "set suggest, edit or execute"),
    ("/budget", "show or set the next turn's USD budget"),
    (
        "/goal",
        "set, view, pause, resume or clear a persistent goal",
    ),
    ("/personality", "set friendly, pragmatic or none"),
    ("/diff", "show tracked workspace and last-run changes"),
    ("/files", "list workspace files locally"),
    ("/mention", "attach a workspace file to future objectives"),
    ("/new", "start a new saved conversation"),
    ("/resume", "list sessions or resume a session UUID"),
    ("/fork", "copy this conversation into a new session"),
    ("/rename", "rename the current saved session"),
    ("/archive", "archive this session and start a new one"),
    ("/delete", "delete this session and start a new one"),
    ("/compact", "retain the last six messages in context"),
    ("/copy", "copy the latest OpenForge output via OSC 52"),
    ("/context", "show conversation context"),
    ("/memories", "inspect repository memory"),
    ("/skills", "list workspace SKILL.md files"),
    ("/mcp", "list servers; use verbose or a server name"),
    ("/apps", "list installed plugin manifests"),
    ("/plugins", "alias for /apps"),
    ("/agent", "inspect task agents and ACP processes"),
    ("/subagents", "alias for /agent"),
    ("/ps", "show live ACP agent processes"),
    ("/stop", "stop an ACP process UUID or all"),
    ("/runs", "list recent runs"),
    ("/tasks", "show tasks for the last run"),
    ("/events", "show events for the last run"),
    ("/usage", "show the last run's budget usage"),
    (
        "/debug-config",
        "show daemon, gateway, model and policy diagnostics",
    ),
    ("/connect", "enter and save Sevi credentials"),
    (
        "/disconnect",
        "disconnect and forget saved Sevi credentials",
    ),
    ("/logout", "disconnect the shared Sevi provider"),
    ("/gateway", "show gateway connection"),
    ("/provider", "show model catalog"),
    ("/clear", "clear conversation context"),
    ("/whoami", "show local user"),
    ("/pwd", "show workspace path"),
    ("/exit", "leave OpenForge"),
    ("/quit", "leave OpenForge"),
];

pub fn help() {
    for (name, description) in COMMANDS {
        println!("{name:14} {description}");
    }
}
fn candidates(input: &str) -> Vec<&'static str> {
    if !input.starts_with('/') || input.contains(char::is_whitespace) {
        return vec![];
    }
    COMMANDS
        .iter()
        .filter(|(name, _)| name.starts_with(input))
        .map(|(name, _)| *name)
        .collect()
}
struct Raw;
impl Drop for Raw {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), event::DisableBracketedPaste);
    }
}

#[derive(Default)]
pub struct Editor {
    history: Vec<String>,
}
impl Editor {
    pub fn read(&mut self) -> Result<Option<String>> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            print!("openforge> ");
            io::stdout().flush()?;
            let mut input = String::new();
            return Ok(if io::stdin().read_line(&mut input)? == 0 {
                None
            } else {
                Some(input.trim().into())
            });
        }
        terminal::enable_raw_mode()?;
        let _raw = Raw;
        execute!(io::stdout(), event::EnableBracketedPaste)?;
        let mut input = String::new();
        let mut selected = 0usize;
        let mut history_at = self.history.len();
        let mut menu = true;
        let mut position = 0usize;
        loop {
            let choices = if menu { candidates(&input) } else { vec![] };
            selected = selected.min(choices.len().saturating_sub(1));
            let width = terminal::size()?.0.saturating_sub(2).max(12) as usize;
            execute!(
                io::stdout(),
                cursor::MoveToColumn(0),
                terminal::Clear(ClearType::FromCursorDown)
            )?;
            let before = render_fragment(&input[..position], width.saturating_sub(12), true);
            let after = render_fragment(
                &input[position..],
                width.saturating_sub(12 + before.chars().count()),
                false,
            );
            print!("openforge> {before}{after}");
            let mut rows = 0;
            let start = selected.saturating_sub(5);
            for (i, name) in choices.iter().enumerate().skip(start).take(6) {
                let description = COMMANDS.iter().find(|(n, _)| n == name).unwrap().1;
                let row = format!(
                    "{} {name:14} {description}",
                    if i == selected { ">" } else { " " }
                );
                print!("\r\n{}", row.chars().take(width).collect::<String>());
                rows += 1;
            }
            if rows > 0 {
                execute!(io::stdout(), cursor::MoveUp(rows))?;
            }
            execute!(
                io::stdout(),
                cursor::MoveToColumn((11 + before.chars().count()) as u16)
            )?;
            io::stdout().flush()?;
            let ev = event::read()?;
            if let Event::Paste(text) = ev {
                let pasted = normalize_paste(&text);
                input.insert_str(position, &pasted);
                position += pasted.len();
                menu = false;
                continue;
            }
            let Event::Key(key) = ev else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Char('d')
                    if key.modifiers.contains(KeyModifiers::CONTROL) && input.is_empty() =>
                {
                    execute!(
                        io::stdout(),
                        cursor::MoveToColumn(0),
                        terminal::Clear(ClearType::FromCursorDown)
                    )?;
                    print!("\r\n");
                    return Ok(None);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                    position = 0;
                    selected = 0;
                    menu = true;
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear();
                    position = 0;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.insert(position, c);
                    position += c.len_utf8();
                    selected = 0;
                    menu = true;
                }
                KeyCode::Backspace => {
                    if let Some((previous, _)) = input[..position].char_indices().next_back() {
                        input.drain(previous..position);
                        position = previous;
                    }
                    selected = 0;
                    menu = true;
                }
                KeyCode::Left => {
                    position = input[..position]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(index, _)| index);
                }
                KeyCode::Right => {
                    if let Some(c) = input[position..].chars().next() {
                        position += c.len_utf8();
                    }
                }
                KeyCode::Home => position = 0,
                KeyCode::End => position = input.len(),
                KeyCode::Delete => {
                    if let Some(c) = input[position..].chars().next() {
                        input.drain(position..position + c.len_utf8());
                    }
                }
                KeyCode::Esc => menu = false,
                KeyCode::Up if !choices.is_empty() => selected = selected.saturating_sub(1),
                KeyCode::Down if !choices.is_empty() => {
                    selected = (selected + 1).min(choices.len() - 1)
                }
                KeyCode::Up => {
                    history_at = history_at.saturating_sub(1);
                    if let Some(value) = self.history.get(history_at) {
                        input = value.clone();
                        position = input.len();
                    }
                    menu = false;
                }
                KeyCode::Down => {
                    history_at = (history_at + 1).min(self.history.len());
                    input = self.history.get(history_at).cloned().unwrap_or_default();
                    position = input.len();
                    menu = false;
                }
                KeyCode::Tab if !choices.is_empty() => {
                    input = format!("{} ", choices[selected]);
                    position = input.len();
                    menu = false;
                }
                KeyCode::Enter => {
                    if !choices.is_empty() {
                        input = choices[selected].into();
                    }
                    execute!(
                        io::stdout(),
                        cursor::MoveToColumn(0),
                        terminal::Clear(ClearType::FromCursorDown)
                    )?;
                    print!("openforge> {input}\r\n");
                    io::stdout().flush()?;
                    if !input.is_empty() {
                        self.history.push(input.clone());
                    }
                    return Ok(Some(input));
                }
                _ => {}
            }
        }
    }
}

fn normalize_paste(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

fn render_fragment(value: &str, maximum: usize, from_end: bool) -> String {
    let rendered = value.replace('\n', "↵").replace('\t', "    ");
    if from_end {
        rendered
            .chars()
            .rev()
            .take(maximum)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        rendered.chars().take(maximum).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn palette_filters_only_commands() {
        assert_eq!(candidates("/res"), vec!["/resume"]);
        assert!(candidates("/resume uuid").is_empty());
        assert!(candidates("ordinary objective").is_empty());
        assert_eq!(candidates("/").len(), COMMANDS.len());
    }

    #[test]
    fn pasted_code_preserves_lines_tabs_and_indentation() {
        assert_eq!(
            normalize_paste("fn main() {\r\n\tprintln!(\"hi\");\r\n}\u{0007}"),
            "fn main() {\n\tprintln!(\"hi\");\n}"
        );
    }
}
