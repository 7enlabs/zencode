//! Line input for the interactive REPL (G6). On a terminal the prompt is read with a line
//! editor: arrow keys move in the line, up and down walk the history, Ctrl-R searches it, and
//! `/vim` switches to vi keys. The history is kept in `~/.zen/repl_history` (owner-only on
//! Unix); a line that starts with a space is not saved, so a secret can be typed that way.
//! When stdin or stdout is not a terminal (pipes, tests, CI) or `ZEN_PLAIN_INPUT` is set, the
//! plain one-line `read_line` is used as before.

use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::{Config, DefaultEditor, EditMode};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

const HISTORY_MAX: usize = 1000;

pub enum ReadOutcome {
    Line(String),
    Interrupted,
    Eof,
}

pub struct LineReader {
    editor: Option<DefaultEditor>,
    history: Option<PathBuf>,
}

/// Where the REPL history lives, under the user's home.
pub fn history_path(home: Option<&std::path::Path>) -> Option<PathBuf> {
    home.map(|h| h.join(".zen").join("repl_history"))
}

/// Split a prompt into the lines printed before it and the last line the editor shows.
pub fn split_prompt(prompt: &str) -> (&str, &str) {
    match prompt.rfind('\n') {
        Some(i) => (&prompt[..=i], &prompt[i + 1..]),
        None => ("", prompt),
    }
}

fn plain_wanted() -> bool {
    std::env::var_os("ZEN_PLAIN_INPUT").is_some()
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
}

impl LineReader {
    pub fn new(home: Option<&std::path::Path>) -> Self {
        if plain_wanted() {
            return Self::plain();
        }
        let config = Config::builder()
            .auto_add_history(false)
            .history_ignore_space(true)
            .build();
        let mut editor = match DefaultEditor::with_config(config) {
            Ok(e) => e,
            Err(_) => return Self::plain(),
        };
        let _ = editor.set_max_history_size(HISTORY_MAX);
        let _ = editor.set_history_ignore_dups(true);
        let history = history_path(home);
        if let Some(p) = &history {
            let _ = editor.load_history(p);
        }
        Self {
            editor: Some(editor),
            history,
        }
    }

    pub fn plain() -> Self {
        Self {
            editor: None,
            history: None,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    pub fn set_vi(&mut self, on: bool) {
        if let Some(e) = self.editor.as_mut() {
            e.set_edit_mode(if on { EditMode::Vi } else { EditMode::Emacs });
        }
    }

    pub fn read(&mut self, prompt: &str) -> ReadOutcome {
        let (before, last) = split_prompt(prompt);
        match self.editor.as_mut() {
            None => {
                print!("{prompt}");
                std::io::stdout().flush().ok();
                let mut line = String::new();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => ReadOutcome::Eof,
                    Ok(_) => ReadOutcome::Line(line),
                }
            }
            Some(editor) => {
                if !before.is_empty() {
                    print!("{before}");
                    std::io::stdout().flush().ok();
                }
                match editor.readline(last) {
                    Ok(line) => ReadOutcome::Line(line),
                    Err(ReadlineError::Interrupted) => ReadOutcome::Interrupted,
                    Err(_) => ReadOutcome::Eof,
                }
            }
        }
    }

    pub fn remember(&mut self, line: &str) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if line.trim().is_empty() || editor.add_history_entry(line).ok() != Some(true) {
            return;
        }
        if let Some(p) = &self.history {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if editor.save_history(p).is_ok() {
                restrict(p);
            }
        }
    }
}

#[cfg(unix)]
fn restrict(p: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prompt_keeps_last_line_for_editor() {
        assert_eq!(split_prompt("> "), ("", "> "));
        assert_eq!(split_prompt("goal: x\n> "), ("goal: x\n", "> "));
    }

    #[test]
    fn history_lives_under_dot_zen() {
        let p = history_path(Some(std::path::Path::new("/h"))).unwrap();
        assert_eq!(p, std::path::Path::new("/h/.zen/repl_history"));
        assert!(history_path(None).is_none());
    }

    #[test]
    fn plain_reader_does_not_edit() {
        let mut r = LineReader::plain();
        assert!(!r.is_editing());
        r.set_vi(true);
        r.remember("ignored");
    }
}
