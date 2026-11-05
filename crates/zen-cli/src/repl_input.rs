//! Line input for the interactive REPL (G6). On a terminal the prompt is read with a line
//! editor: arrow keys move in the line, up and down walk the history, Ctrl-R searches it, and
//! `/vim` switches to vi keys. The history is kept in `~/.zen/repl_history` (owner-only on
//! Unix); a line that starts with a space is not saved, so a secret can be typed that way.
//! When stdin or stdout is not a terminal (pipes, tests, CI) or `ZEN_PLAIN_INPUT` is set, the
//! plain one-line `read_line` is used as before.

use rustyline::completion::{Completer, Pair};
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{CompletionType, Config, Context, EditMode, Editor, Helper};
use std::borrow::Cow;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

const HISTORY_MAX: usize = 1000;

pub enum ReadOutcome {
    Line(String),
    Interrupted,
    Eof,
}

pub struct LineReader {
    editor: Option<Editor<SlashHelper, DefaultHistory>>,
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

/// Prompt helper for slash commands (S9 pattern 1): typing a slash prefix
/// offers the matching commands right at the typing point. Tab lists every
/// match with its one-line description, the best match is ghosted after the
/// cursor as a hint, and Esc cancels the list; both keys are named in the
/// REPL banner.
#[derive(Clone, Copy, Default)]
pub struct SlashHelper;

/// The word being completed is a slash command when the text before the
/// cursor starts with a slash and has no whitespace yet (arguments are not
/// completed).
fn slash_prefix(line: &str, pos: usize) -> Option<&str> {
    let before = &line[..pos];
    if before.starts_with('/') && !before.contains(char::is_whitespace) {
        Some(before)
    } else {
        None
    }
}

/// Completion candidates for a slash prefix, in catalog order.
pub fn slash_candidates(prefix: &str) -> Vec<Pair> {
    crate::commands::slash_command_catalog()
        .into_iter()
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, desc)| Pair {
            display: format!("{name}  {desc}"),
            replacement: name.to_string(),
        })
        .collect()
}

/// Ghost text for the line being typed: the remainder of the first catalog
/// command that extends what is already typed.
pub fn slash_hint(line: &str, pos: usize) -> Option<String> {
    let prefix = slash_prefix(line, pos)?;
    if pos != line.len() {
        return None;
    }
    crate::commands::slash_command_catalog()
        .into_iter()
        .map(|(name, _)| name)
        .find(|name| name.len() > prefix.len() && name.starts_with(prefix))
        .map(|name| name[prefix.len()..].to_string())
}

impl Completer for SlashHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        match slash_prefix(line, pos) {
            Some(prefix) => Ok((0, slash_candidates(prefix))),
            None => Ok((pos, Vec::new())),
        }
    }
}

impl Hinter for SlashHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        slash_hint(line, pos)
    }
}

impl Highlighter for SlashHelper {
    // The ghosted suggestion is the selected item at the typing point, so it
    // is painted bold (brighter) instead of the default dim ghost style.
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if crate::repl_polish::Theme::colors_disabled() {
            Cow::Borrowed(hint)
        } else {
            Cow::Owned(format!("\u{1b}[1m{hint}\u{1b}[0m"))
        }
    }
}

impl Validator for SlashHelper {}

impl Helper for SlashHelper {}

fn plain_wanted() -> bool {
    std::env::var_os("ZEN_PLAIN_INPUT").is_some()
        || !std::io::stdin().is_terminal()
        || !std::io::stdout().is_terminal()
}

/// Editor config for the interactive prompt. `CompletionType::List` is
/// what makes Esc cancel the completion list (rustyline 18: a lone Esc
/// is `Cmd::Abort`; while a list is shown it is handed back to the main
/// loop, which ignores it, so the list is dismissed and the typed line
/// is kept). Keep this in one place so the test below can pin it.
fn editor_config() -> Config {
    Config::builder()
        .auto_add_history(false)
        .history_ignore_space(true)
        .completion_type(CompletionType::List)
        .build()
}

impl LineReader {
    pub fn new(home: Option<&std::path::Path>) -> Self {
        if plain_wanted() {
            return Self::plain();
        }
        let config = editor_config();
        let mut editor = match Editor::<SlashHelper, DefaultHistory>::with_config(config) {
            Ok(e) => e,
            Err(_) => return Self::plain(),
        };
        editor.set_helper(Some(SlashHelper));
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
    fn slash_candidates_match_prefix_with_descriptions() {
        let all = slash_candidates("/");
        assert!(all.len() >= 40, "catalog too small: {}", all.len());
        assert!(all.iter().all(|p| p.replacement.starts_with('/')));
        let mo = slash_candidates("/mo");
        assert_eq!(mo.len(), 1);
        assert_eq!(mo[0].replacement, "/model");
        assert!(
            mo[0].display.contains("switch"),
            "display: {}",
            mo[0].display
        );
        assert!(slash_candidates("/zzz").is_empty());
    }

    #[test]
    fn slash_hint_extends_first_match_only_at_line_end() {
        assert_eq!(slash_hint("/mo", 3).as_deref(), Some("del"));
        assert_eq!(slash_hint("/model", 6), None);
        assert_eq!(slash_hint("hello", 5), None);
        assert_eq!(slash_hint("/mo x", 5), None);
        assert_eq!(slash_hint("/mo", 2), None);
    }

    #[test]
    fn esc_cancels_completion_list_because_config_is_list() {
        // Tab lists the slash-command matches; Esc must dismiss the
        // list without touching the typed line. That only holds with
        // CompletionType::List: in Circular mode Tab cycles inline
        // instead of listing, so Esc would behave differently. If this
        // config ever changes, the banner promise in repl.rs ("Esc
        // cancels") breaks with it.
        assert_eq!(editor_config().completion_type(), CompletionType::List);
    }

    #[test]
    fn plain_reader_does_not_edit() {
        let mut r = LineReader::plain();
        assert!(!r.is_editing());
        r.set_vi(true);
        r.remember("ignored");
    }
}
