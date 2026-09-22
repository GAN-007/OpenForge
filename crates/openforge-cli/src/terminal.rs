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
    (
        "/init",
        "create project instructions without overwriting them",
    ),
    (
        "/review",
        "review tracked changes using a read-only model run",
    ),
    ("/status", "session, workspace and last run"),
    ("/model", "inspect available models and active gateway"),
    (
        "/permissions",
        "show policy; add a path to select another policy",
    ),
    ("/plan", "switch to planning without execution"),
    ("/mode", "set suggest, edit or execute"),
    ("/budget", "show or set the next turn's USD budget"),
    ("/diff", "show uncommitted workspace changes"),
    ("/files", "list workspace files locally"),
    ("/new", "start a new saved conversation"),
    ("/resume", "list sessions or resume a session UUID"),
    ("/fork", "copy this conversation into a new session"),
    ("/compact", "retain the last six messages in context"),
    ("/context", "show conversation context"),
    ("/mcp", "list servers; add a server name to discover tools"),
    ("/apps", "list installed plugins"),
    ("/runs", "list recent runs"),
    ("/tasks", "show tasks for the last run"),
    ("/events", "show events for the last run"),
    ("/connect", "enter and save Sevi credentials"),
    (
        "/disconnect",
        "disconnect and forget saved Sevi credentials",
    ),
    ("/gateway", "show gateway connection"),
    ("/provider", "show model catalog"),
    ("/clear", "clear conversation context"),
    ("/whoami", "show local user"),
    ("/pwd", "show workspace path"),
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
            let before: String = input[..position]
                .chars()
                .rev()
                .take(width.saturating_sub(12))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let after: String = input[position..]
                .chars()
                .take(width.saturating_sub(12 + before.chars().count()))
                .collect();
            print!(
                "openforge> {}{}",
                display_input(&before),
                display_input(&after)
            );
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
                let pasted = pasted_text(&text);
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
                KeyCode::Enter
                    if key
                        .modifiers
                        .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
                {
                    input.insert(position, '\n');
                    position += 1;
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
                    print!("openforge> {}\r\n", input.replace('\n', "\r\n"));
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

fn pasted_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect()
}
fn display_input(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\n' => '↵',
            '\t' => '→',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pasted_code_preserves_lines_and_indentation_without_terminal_controls() {
        assert_eq!(
            pasted_text("fn main() {\r\n\tcall();\n}\u{1b}"),
            "fn main() {\n\tcall();\n}"
        );
        assert_eq!(display_input("a\n\tb"), "a↵→b");
    }
    #[test]
    fn palette_filters_only_commands() {
        assert_eq!(candidates("/res"), vec!["/resume"]);
        assert!(candidates("/resume uuid").is_empty());
        assert!(candidates("ordinary objective").is_empty());
        assert_eq!(candidates("/").len(), COMMANDS.len());
    }
}
