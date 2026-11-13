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
    prompt_sgr: Option<u8>,
    /// Model rows for /model Tab completion (B2). The helper is
    /// re-installed on every read, so the rows live here.
    models: Vec<(String, String, String)>,
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
#[derive(Clone, Default)]
pub struct SlashHelper {
    /// SGR code that paints the trailing "> " of the prompt, or None
    /// for no styling. Taken from the active theme on every read; the
    /// prompt string itself is always handed to rustyline unstyled
    /// (see `editor_prompt`).
    pub prompt_sgr: Option<u8>,

    /// Model rows for `/model` argument completion (B2): (short,
    /// provider, id). Empty when /model has never listed models.
    pub models: Vec<(String, String, String)>,
}

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

/// Completion candidates for a `/model` argument: rows are (short,
/// provider, id). Matches a case-insensitive prefix of the short name
/// or the full model id; an empty prefix lists every row. The
/// replacement is the full id, which `pick_model` accepts.
pub fn model_candidates(prefix: &str, models: &[(String, String, String)]) -> Vec<Pair> {
    let lower = prefix.to_lowercase();
    models
        .iter()
        .filter(|(short, _, id)| {
            short.to_lowercase().starts_with(&lower) || id.to_lowercase().starts_with(&lower)
        })
        .map(|(short, provider, id)| Pair {
            display: format!("{short}  {provider}"),
            replacement: id.clone(),
        })
        .collect()
}

/// Ghost text for a `/model` argument: the remainder of the first
/// model id that extends what is already typed. Only id prefixes
/// ghost - the ghost must match what Tab inserts (the id); a short
/// name the user typed is still completed by Tab, just not ghosted.
pub fn model_hint(line: &str, pos: usize, models: &[(String, String, String)]) -> Option<String> {
    let arg = line.strip_prefix("/model ")?;
    if pos != line.len() || arg.contains(char::is_whitespace) {
        return None;
    }
    let lower = arg.to_lowercase();
    models
        .iter()
        .map(|(_, _, id)| id)
        .find(|id| id.len() > arg.len() && id.to_lowercase().starts_with(&lower))
        .map(|id| id[arg.len()..].to_string())
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
        // Core-perfect phase 2 (B2): Tab completes the /model argument
        // from the rows /model listed. No arrow-key menu: rustyline
        // 18 has no completion-list navigation and the repo has no
        // menu library - that still needs a dependency decision.
        if line.starts_with("/model ") && pos >= 7 {
            let arg = &line[7..pos];
            if !arg.contains(char::is_whitespace) {
                return Ok((7, model_candidates(arg, &self.models)));
            }
        }
        match slash_prefix(line, pos) {
            Some(prefix) => Ok((0, slash_candidates(prefix))),
            None => Ok((pos, Vec::new())),
        }
    }
}

impl Hinter for SlashHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &Context<'_>) -> Option<String> {
        slash_hint(line, pos).or_else(|| model_hint(line, pos, &self.models))
    }
}

impl Highlighter for SlashHelper {
    /// Paint only the trailing "> " marker of the prompt. This is the
    /// one place prompt styling may live: rustyline measures the
    /// prompt string it is given, so styling baked into that string
    /// counts as printable columns and corrupts every redraw (on
    /// Windows, up to erasing freshly typed characters). The layout
    /// math always sees the unstyled prompt; only the displayed copy
    /// returned here carries the SGR codes.
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> Cow<'b, str> {
        match self.prompt_sgr {
            Some(code) if prompt.ends_with("> ") => {
                let head = &prompt[..prompt.len() - 2];
                Cow::Owned(format!("{head}\x1b[{code}m> \x1b[0m"))
            }
            _ => Cow::Borrowed(prompt),
        }
    }

    // The ghost hint must be returned verbatim, with no ANSI styling.
    // rustyline 18 has no default dim ghost style to replace: its
    // default highlight_hint already returns the hint as-is, and its
    // layout math counts the bytes of any ANSI wrapper as visible
    // columns. Wrapping the hint in bold ("\x1b[1m...\x1b[0m") added
    // 6 phantom columns, so once the hint vanished the redraw left a
    // 6-column gap inside the typed line.
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        Cow::Borrowed(hint)
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

/// Remove ANSI escape sequences from `s`: CSI sequences (ESC [ ...
/// final byte in @ through ~) and any other two-byte ESC sequence.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some('[') = chars.next() {
                for c2 in chars.by_ref() {
                    if ('@'..='~').contains(&c2) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The prompt actually handed to the line editor: the last prompt
/// line with all styling removed. rustyline's contract is that the
/// prompt passed to `readline` is unstyled - its layout code counts
/// any ANSI bytes in it as printable columns (a cyan "> " measured 11
/// columns instead of 2), and on Windows every redraw clears and
/// repositions from that wrong layout, which is how typed characters
/// ended up shifted or erased instead of echoing. The styling is
/// re-applied for display only, by `SlashHelper::highlight_prompt`.
pub fn editor_prompt(last: &str) -> String {
    strip_ansi(last)
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
        editor.set_helper(Some(SlashHelper::default()));
        let _ = editor.set_max_history_size(HISTORY_MAX);
        let _ = editor.set_history_ignore_dups(true);
        let history = history_path(home);
        if let Some(p) = &history {
            let _ = editor.load_history(p);
        }
        Self {
            editor: Some(editor),
            history,
            prompt_sgr: None,
            models: Vec::new(),
        }
    }

    pub fn plain() -> Self {
        Self {
            editor: None,
            history: None,
            prompt_sgr: None,
            models: Vec::new(),
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editor.is_some()
    }

    /// Record the active theme's prompt styling for the next read.
    /// The helper is re-installed with this value in `read`, because
    /// the theme can change mid-session.
    pub fn set_prompt_style(&mut self, theme: crate::repl_polish::Theme) {
        self.prompt_sgr = theme.prompt_sgr();
    }

    /// Record the model rows `/model` listed, for Tab completion of
    /// the next `/model` argument.
    pub fn set_model_rows(&mut self, rows: &[(String, String, String)]) {
        self.models = rows.to_vec();
    }

    pub fn set_vi(&mut self, on: bool) {
        if let Some(e) = self.editor.as_mut() {
            e.set_edit_mode(if on { EditMode::Vi } else { EditMode::Emacs });
        }
    }

    pub fn read(&mut self, prompt: &str) -> ReadOutcome {
        let (before, last) = split_prompt(prompt);
        let prompt_sgr = self.prompt_sgr;
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
                editor.set_helper(Some(SlashHelper {
                    prompt_sgr,
                    models: self.models.clone(),
                }));
                match editor.readline(&editor_prompt(last)) {
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
    fn highlight_hint_has_no_ansi_escapes() {
        // Regression: the ghost hint used to be wrapped in bold ANSI
        // ("\x1b[1m...\x1b[0m"). rustyline counts those 6 bytes as
        // visible columns when it lays the line out, so once the hint
        // vanished the redraw left a 6-column gap inside the typed
        // line ("/d/" + 6 spaces + "dsadshjdh"). The hint must be
        // returned verbatim: rustyline 18 has no default dim ghost
        // style, its default highlight_hint already returns the hint
        // as-is.
        let hint = SlashHelper::default().highlight_hint("iff");
        assert!(
            !hint.contains("\x1b"),
            "hint must not contain ANSI escapes, got: {hint:?}"
        );
        assert_eq!(hint.as_ref(), "iff");
    }

    #[test]
    fn strip_ansi_removes_sgr_sequences() {
        assert_eq!(strip_ansi("\x1b[36m> \x1b[0m"), "> ");
        assert_eq!(strip_ansi("\x1b[38;5;27mblue\x1b[0m"), "blue");
        assert_eq!(strip_ansi("plain"), "plain");
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn editor_prompt_is_unstyled_with_true_width() {
        // Regression (no-echo on Windows): the prompt handed to
        // rustyline used to carry the cyan SGR codes, so rustyline
        // laid the line out as if the prompt were 11 columns wide
        // instead of 2 and every redraw on Windows cleared and
        // repositioned from that wrong layout.
        let p = editor_prompt("\x1b[36m> \x1b[0m");
        assert!(!p.contains('\x1b'), "prompt must be unstyled: {p:?}");
        assert_eq!(p, "> ");
        assert_eq!(editor_prompt("[PLAN] \x1b[36m> \x1b[0m"), "[PLAN] > ");
        assert_eq!(editor_prompt("... "), "... ");
    }

    #[test]
    fn highlight_prompt_paints_only_trailing_marker() {
        let h = SlashHelper {
            prompt_sgr: Some(36),
            models: vec![],
        };
        assert_eq!(h.highlight_prompt("> ", true).as_ref(), "\x1b[36m> \x1b[0m");
        assert_eq!(
            h.highlight_prompt("[PLAN] > ", true).as_ref(),
            "[PLAN] \x1b[36m> \x1b[0m"
        );
        assert_eq!(h.highlight_prompt("... ", true).as_ref(), "... ");
        let plain = SlashHelper {
            prompt_sgr: None,
            models: vec![],
        };
        assert_eq!(plain.highlight_prompt("> ", true).as_ref(), "> ");
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
    fn model_candidates_match_short_or_id_prefix() {
        let models = vec![
            (
                "nemo".to_string(),
                "openrouter".to_string(),
                "nvidia/nemotron-nano-9b-v2:free".to_string(),
            ),
            (
                "flash".to_string(),
                "deploy".to_string(),
                "deepseek-ai/deepseek-v4.1-flash-1017".to_string(),
            ),
        ];
        let all = model_candidates("", &models);
        assert_eq!(all.len(), 2);
        let by_short = model_candidates("nem", &models);
        assert_eq!(by_short.len(), 1);
        assert_eq!(by_short[0].replacement, "nvidia/nemotron-nano-9b-v2:free");
        let by_id = model_candidates("deepseek-ai/", &models);
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].replacement, "deepseek-ai/deepseek-v4.1-flash-1017");
        assert!(model_candidates("zzz", &models).is_empty());
    }

    #[test]
    fn model_hint_completes_id_prefix_only() {
        let models = vec![(
            "flash".to_string(),
            "deploy".to_string(),
            "deepseek-ai/deepseek-v4.1-flash-1017".to_string(),
        )];
        let line = "/model deepseek-ai/deep";
        assert_eq!(
            model_hint(line, line.len(), &models).as_deref(),
            Some("seek-v4.1-flash-1017")
        );
        let full = "/model deepseek-ai/deepseek-v4.1-flash-1017";
        assert_eq!(model_hint(full, full.len(), &models), None);
        let empty = "/model ";
        assert_eq!(
            model_hint(empty, empty.len(), &models).as_deref(),
            Some("deepseek-ai/deepseek-v4.1-flash-1017")
        );
        // A short-name prefix is completed by Tab, not ghosted.
        let short = "/model flash";
        assert_eq!(model_hint(short, short.len(), &models), None);
    }

    #[test]
    fn model_hint_is_none_when_cursor_is_mid_line() {
        // The ghost must only offer the remainder past the cursor at the
        // end of the line: with the cursor mid-line the `||` guard returns
        // None, while `&&` would keep ghosting (mutant repl_input.rs:116).
        let models = vec![(
            "flash".to_string(),
            "deploy".to_string(),
            "deepseek-ai/deepseek-v4.1-flash-1017".to_string(),
        )];
        let line = "/model deepseek-ai/deep";
        assert_eq!(model_hint(line, 7, &models), None);
    }

    #[test]
    fn slash_helper_complete_offers_model_rows() {
        // Killing the whole-body mutant (repl_input.rs:154): Tab on a
        // `/model` argument must return the model rows at start 7, not an
        // empty list.
        use rustyline::history::DefaultHistory;
        let helper = SlashHelper {
            prompt_sgr: None,
            models: vec![(
                "flash".to_string(),
                "deploy".to_string(),
                "deepseek-ai/deepseek-v4.1-flash-1017".to_string(),
            )],
        };
        let history = DefaultHistory::new();
        let ctx = Context::new(&history);
        let (start, candidates) = helper.complete("/model deep", 11, &ctx).expect("complete");
        assert_eq!(start, 7);
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].replacement,
            "deepseek-ai/deepseek-v4.1-flash-1017"
        );
    }

    #[test]
    fn plain_reader_does_not_edit() {
        let mut r = LineReader::plain();
        assert!(!r.is_editing());
        r.set_vi(true);
        r.remember("ignored");
    }
}
