//! Line input for the plain (REPL) modes, with a history of earlier lines.
//!
//! In a terminal, lines are edited in raw mode so `Up` and `Down` can bring back
//! earlier guesses and commands, as a shell does. It's a small editor over
//! crossterm (already a dependency) rather than a line-editing crate: plain mode
//! needs only short single lines. Piped input is read as plain lines, since
//! there are no keys to handle and raw mode would fail.

use std::io::{self, IsTerminal, Write};

use crossterm::cursor::MoveToColumn;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{self, Clear, ClearType};
use crossterm::{execute, queue};

/// Reads lines from stdin, editing them in place when it's a terminal.
pub struct LineReader {
    history: Vec<String>,
    interactive: bool,
}

impl Default for LineReader {
    fn default() -> Self {
        Self::new()
    }
}

/// Raw mode for one line's editing, ended when dropped (on error too).
struct RawMode;

impl RawMode {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

impl LineReader {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            interactive: io::stdin().is_terminal() && io::stdout().is_terminal(),
        }
    }

    /// Shows `prompt` and reads a line, without its newline. `None` at the end of
    /// input: end of file, or `Ctrl+D` / `Ctrl+C` in a terminal.
    pub fn read_line(&mut self, prompt: &str) -> io::Result<Option<String>> {
        let line = if self.interactive {
            self.edit(prompt)?
        } else {
            print!("{prompt}");
            io::stdout().flush()?;
            let mut line = String::new();
            if io::stdin().read_line(&mut line)? == 0 {
                None
            } else {
                Some(line.trim_end_matches(['\n', '\r']).to_string())
            }
        };
        if let Some(line) = &line {
            remember(&mut self.history, line);
        }
        Ok(line)
    }

    fn edit(&mut self, prompt: &str) -> io::Result<Option<String>> {
        let mut out = io::stdout();
        let _raw = RawMode::enter()?;
        let mut editor = Editor::default();
        redraw(&mut out, prompt, &editor)?;
        loop {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match editor.handle(key, &self.history) {
                Outcome::Editing => redraw(&mut out, prompt, &editor)?,
                Outcome::Submit(line) => {
                    execute!(out, crossterm::style::Print("\r\n"))?;
                    return Ok(Some(line));
                }
                Outcome::End => {
                    // The caller prints the newline that ends the session.
                    return Ok(None);
                }
            }
        }
    }
}

/// Adds `line` to the history, unless it's blank or repeats the latest entry.
fn remember(history: &mut Vec<String>, line: &str) {
    let line = line.trim();
    if !line.is_empty() && history.last().is_none_or(|last| last != line) {
        history.push(line.to_string());
    }
}

/// Rewrites the line in place, leaving the cursor where the editor's is.
fn redraw(out: &mut impl Write, prompt: &str, editor: &Editor) -> io::Result<()> {
    let columns = terminal::size().map_or(80, |(w, _)| usize::from(w));
    let (text, cursor) = visible(prompt, editor, columns);
    queue!(
        out,
        MoveToColumn(0),
        crossterm::style::Print(prompt),
        crossterm::style::Print(text),
        Clear(ClearType::UntilNewLine),
        MoveToColumn(u16::try_from(cursor).unwrap_or(u16::MAX)),
    )?;
    out.flush()
}

/// Width of `text` in terminal cells (a CJK character or emoji takes two).
fn cells(text: &str) -> usize {
    ratatui::text::Span::raw(text).width()
}

/// The part of the line to show after `prompt` in a terminal `columns` wide,
/// and the cursor's column. A line too long for one row scrolls sideways to keep
/// the cursor in view, since a line that wrapped onto a second row would be
/// redrawn on the wrong one. Widths are counted in cells, not characters.
fn visible(prompt: &str, editor: &Editor, columns: usize) -> (String, usize) {
    let prompt_cells = cells(prompt);
    // A cell is kept free at the end, so the cursor never sits past the edge.
    let room = columns.saturating_sub(prompt_cells + 1).max(1);
    let width = |c: &char| cells(&c.to_string());
    // Start as late as needed for the text up to the cursor to fit.
    let mut start = 0;
    while editor.buf[start..editor.cursor]
        .iter()
        .map(width)
        .sum::<usize>()
        > room
    {
        start += 1;
    }
    let mut used = 0;
    let shown: String = editor.buf[start..]
        .iter()
        .take_while(|c| {
            used += width(c);
            used <= room
        })
        .collect();
    let cursor = prompt_cells
        + editor.buf[start..editor.cursor]
            .iter()
            .map(width)
            .sum::<usize>();
    (shown, cursor)
}

/// What a key did to the line being edited.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Editing,
    Submit(String),
    End,
}

/// One line being edited: its text, the cursor, and where in the history `Up`
/// and `Down` have got to. Kept apart from the terminal so it can be tested.
#[derive(Debug, Default)]
struct Editor {
    buf: Vec<char>,
    cursor: usize,
    /// Index of the history entry shown, while browsing it.
    browsing: Option<usize>,
    /// What was typed before browsing began, restored by `Down` past the newest entry.
    draft: Vec<char>,
}

impl Editor {
    fn set(&mut self, text: Vec<char>) {
        self.cursor = text.len();
        self.buf = text;
    }

    fn handle(&mut self, key: KeyEvent, history: &[String]) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return Outcome::Submit(self.buf.iter().collect()),
            KeyCode::Char('c') if ctrl => return Outcome::End,
            KeyCode::Char('d') if ctrl && self.buf.is_empty() => return Outcome::End,
            KeyCode::Char('d') if ctrl => {
                if self.cursor < self.buf.len() {
                    self.buf.remove(self.cursor);
                }
            }
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.buf.len(),
            KeyCode::Char('u') if ctrl => {
                self.buf.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.buf.truncate(self.cursor),
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(c) => {
                self.buf.insert(self.cursor, c);
                self.cursor += 1;
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.buf.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.buf.len() => {
                self.buf.remove(self.cursor);
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.buf.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.buf.len(),
            KeyCode::Up if !history.is_empty() => {
                let pos = match self.browsing {
                    None => {
                        self.draft = self.buf.clone();
                        history.len() - 1
                    }
                    Some(pos) => pos.saturating_sub(1),
                };
                self.browsing = Some(pos);
                self.set(history[pos].chars().collect());
            }
            KeyCode::Down => match self.browsing {
                Some(pos) if pos + 1 < history.len() => {
                    self.browsing = Some(pos + 1);
                    self.set(history[pos + 1].chars().collect());
                }
                Some(_) => {
                    self.browsing = None;
                    let draft = std::mem::take(&mut self.draft);
                    self.set(draft);
                }
                None => {}
            },
            _ => {}
        }
        Outcome::Editing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn type_text(editor: &mut Editor, text: &str, history: &[String]) {
        for c in text.chars() {
            editor.handle(key(KeyCode::Char(c)), history);
        }
    }

    fn text(editor: &Editor) -> String {
        editor.buf.iter().collect()
    }

    #[test]
    fn edits_in_place() {
        let mut e = Editor::default();
        type_text(&mut e, "crne", &[]);
        e.handle(key(KeyCode::Left), &[]);
        e.handle(key(KeyCode::Left), &[]);
        type_text(&mut e, "a", &[]);
        assert_eq!(text(&e), "crane");
        e.handle(key(KeyCode::Home), &[]);
        e.handle(key(KeyCode::Delete), &[]);
        e.handle(key(KeyCode::End), &[]);
        e.handle(key(KeyCode::Backspace), &[]);
        assert_eq!(text(&e), "ran");
        e.handle(key(KeyCode::Left), &[]);
        e.handle(ctrl('k'), &[]);
        assert_eq!(text(&e), "ra");
        e.handle(ctrl('u'), &[]);
        assert_eq!(text(&e), "");
        assert_eq!(
            e.handle(key(KeyCode::Enter), &[]),
            Outcome::Submit(String::new())
        );
    }

    #[test]
    fn browses_history_and_restores_the_draft() {
        let history: Vec<String> = ["crane", ":help", "slate"].map(String::from).to_vec();
        let mut e = Editor::default();
        type_text(&mut e, "dra", &history);
        e.handle(key(KeyCode::Up), &history);
        assert_eq!(text(&e), "slate");
        e.handle(key(KeyCode::Up), &history);
        e.handle(key(KeyCode::Up), &history);
        // Past the oldest entry, Up stays on it.
        e.handle(key(KeyCode::Up), &history);
        assert_eq!(text(&e), "crane");
        assert_eq!(e.cursor, 5);
        e.handle(key(KeyCode::Down), &history);
        assert_eq!(text(&e), ":help");
        e.handle(key(KeyCode::Down), &history);
        e.handle(key(KeyCode::Down), &history);
        assert_eq!(text(&e), "dra");
        // An entry brought back can be edited and sent.
        e.handle(key(KeyCode::Up), &history);
        e.handle(key(KeyCode::Backspace), &history);
        assert_eq!(
            e.handle(key(KeyCode::Enter), &history),
            Outcome::Submit("slat".to_string())
        );
    }

    #[test]
    fn ctrl_d_and_ctrl_c_end_input() {
        let mut e = Editor::default();
        assert_eq!(e.handle(ctrl('d'), &[]), Outcome::End);
        type_text(&mut e, "ab", &[]);
        e.handle(key(KeyCode::Home), &[]);
        assert_eq!(e.handle(ctrl('d'), &[]), Outcome::Editing);
        assert_eq!(text(&e), "b");
        assert_eq!(e.handle(ctrl('c'), &[]), Outcome::End);
    }

    #[test]
    fn long_lines_scroll_to_keep_the_cursor_in_view() {
        let mut e = Editor::default();
        type_text(&mut e, "abcdefghijklmnopqrstuvwxyz", &[]);
        // "> " and a 12-column terminal leave 9 cells for text.
        let (shown, cursor) = visible("> ", &e, 12);
        assert_eq!(shown, "rstuvwxyz");
        assert_eq!(cursor, 11);
        // Back at the start, the start shows.
        e.handle(key(KeyCode::Home), &[]);
        let (shown, cursor) = visible("> ", &e, 12);
        assert_eq!(shown, "abcdefghi");
        assert_eq!(cursor, 2);
        // A short line is shown whole.
        let mut short = Editor::default();
        type_text(&mut short, "crane", &[]);
        assert_eq!(visible("> ", &short, 80), ("crane".to_string(), 7));
    }

    #[test]
    fn wide_characters_take_two_cells() {
        let mut e = Editor::default();
        type_text(&mut e, "日本", &[]);
        let (shown, cursor) = visible("> ", &e, 80);
        assert_eq!(shown, "日本");
        assert_eq!(cursor, 2 + 4);
    }

    #[test]
    fn history_skips_blanks_and_repeats() {
        let mut history = Vec::new();
        for line in ["crane", "", "  ", "crane", " slate ", "crane"] {
            remember(&mut history, line);
        }
        assert_eq!(history, ["crane", "slate", "crane"]);
    }
}
