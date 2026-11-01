use std::io::{self, stdout};
use std::ops::ControlFlow;
use std::panic;

use crossterm::cursor::Show;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

use crate::dict;
use crate::game::{
    supports_color, GameMode, GameState, GameStatus, LetterState, ScratchState, MAX_WORDLE_GUESSES,
};
use crate::help::{draw_help_page, HelpLine, HelpPage, HelpSection};
use crate::save::{Autosave, Notice};
use crate::text::plural;

/// Okabe-Ito colorblind-safe palette
pub const COLOR_ACCENT: Color = Color::Rgb(86, 180, 233); // Sky Blue (#56B4E9)
pub const COLOR_POSITIVE: Color = Color::Rgb(0, 158, 115); // Bluish-Green (#009E73)
pub const COLOR_NEGATIVE: Color = Color::Rgb(213, 94, 0); // Vermilion / Orange-Red (#D55E00)
pub const COLOR_WARNING: Color = Color::Rgb(230, 159, 0); // Orange (#E69F00)

/// Bold text in `color`.
pub fn bold(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// A badge: bold black text on `bg`.
pub fn black_on(bg: Color) -> Style {
    bold(Color::Black).bg(bg)
}

/// The background every TUI paints: true black (RGB 0, 0, 0, pixels off) rather
/// than the terminal's own background or its palette's "black", which many
/// themes make a dark gray.
pub const COLOR_BACKGROUND: Color = Color::Rgb(0, 0, 0);

/// Gives every cell of `buf` without a background of its own the true-black
/// [`COLOR_BACKGROUND`]. Run after a frame is drawn, so it reaches the cells a
/// modal cleared back to the terminal's default too.
pub fn paint_background(buf: &mut ratatui::buffer::Buffer) {
    for cell in buf.content.iter_mut() {
        if cell.bg == Color::Reset {
            cell.set_bg(COLOR_BACKGROUND);
        }
    }
}

/// Minimum recommended terminal width in columns: narrower than this, a TUI
/// shows a note instead. Low enough for a phone held upright.
pub(crate) const MIN_RECOMMENDED_WIDTH: u16 = 36;

/// Below this width a TUI stacks its panels top to bottom rather than side by
/// side, as on a phone held upright or a narrow window.
pub(crate) const NARROW_WIDTH: u16 = 80;

/// A TUI's footer of clickable buttons, `(action, key, label)` each, and where
/// each was last drawn, for the mouse.
pub(crate) struct Footer<A: 'static> {
    actions: &'static [(A, &'static str, &'static str)],
    pub(crate) buttons: Vec<Rect>,
}

impl<A: Copy> Footer<A> {
    pub(crate) fn new(actions: &'static [(A, &'static str, &'static str)]) -> Self {
        Self {
            actions,
            buttons: vec![Rect::default(); actions.len()],
        }
    }

    /// Rows of buttons a terminal `width` wide needs: as few as let each show
    /// its "[key] label" whole inside its border.
    fn rows(&self, width: u16) -> usize {
        // "[key] label" plus a border either side.
        let need = self
            .actions
            .iter()
            .map(|(_, key, label)| key.chars().count() + 1 + label.chars().count() + 2)
            .max()
            .unwrap_or(1);
        let fit = (usize::from(width) / need).clamp(1, self.actions.len().max(1));
        self.actions.len().div_ceil(fit).max(1)
    }

    /// Height of the footer on a terminal `width` wide.
    pub(crate) fn height(&self, width: u16) -> u16 {
        self.rows(width) as u16 * 3
    }

    /// Draws the buttons in `area`, the rows as evenly filled as can be (so
    /// seven buttons go 4 + 3, not 6 + 1), and notes where each went.
    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect) {
        let rows = self.rows(area.width);
        let per_row = self.actions.len().div_ceil(rows).max(1);
        let row_areas = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(3); rows])
            .split(area);
        self.buttons = self
            .actions
            .chunks(per_row)
            .zip(row_areas.iter())
            .flat_map(|(row, &row_area)| {
                Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints(vec![Constraint::Ratio(1, row.len() as u32); row.len()])
                    .split(row_area)
                    .to_vec()
            })
            .collect();
        for (&(_, key, label), &button) in self.actions.iter().zip(&self.buttons) {
            let btn = Paragraph::new(Line::from(vec![
                Span::styled(format!("{key} "), bold(COLOR_ACCENT)),
                Span::styled(label, Style::default().fg(Color::White)),
            ]))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL));
            f.render_widget(btn, button);
        }
    }

    /// Forgets where the buttons were, when the screen doesn't show them.
    pub(crate) fn clear(&mut self) {
        self.buttons.fill(Rect::default());
    }

    /// The action of the button at terminal cell (`col`, `row`), if any.
    pub(crate) fn hit(&self, col: u16, row: u16) -> Option<A> {
        let i = self
            .buttons
            .iter()
            .position(|&r| rect_contains(r, col, row))?;
        Some(self.actions[i].0)
    }
}

/// On a terminal narrower than [`MIN_RECOMMENDED_WIDTH`], fills it with a note
/// saying so, in a box titled `title` that suggests `command --plain`, and
/// returns `true`; otherwise draws nothing.
pub(crate) fn draw_too_narrow(f: &mut Frame, title: &str, command: &str) -> bool {
    let width = f.area().width;
    if width >= MIN_RECOMMENDED_WIDTH {
        return false;
    }
    let warning = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "Terminal Width Too Narrow",
            bold(COLOR_WARNING),
        )),
        Line::from(""),
        Line::from(format!(
            "Current width: {width} columns (minimum recommended: {MIN_RECOMMENDED_WIDTH} columns)."
        )),
        Line::from(format!(
            "Please widen your terminal window, or run `{command} --plain`."
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press [Esc] or [Ctrl+C] to quit.",
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true })
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title.to_string())
            .border_style(Style::default().fg(COLOR_WARNING)),
    );
    f.render_widget(warning, f.area());
    true
}

/// A game screen's `[header, body, footer]` areas, the body at least
/// `min_body` rows, and whether the screen is narrow (below [`NARROW_WIDTH`]),
/// where the header gets a second line to wrap onto rather than be cut.
pub(crate) fn screen_areas<A: Copy>(
    area: Rect,
    footer: &Footer<A>,
    min_body: u16,
) -> ([Rect; 3], bool) {
    let narrow = area.width < NARROW_WIDTH;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if narrow { 4 } else { 3 }),
            Constraint::Min(min_body),
            Constraint::Length(footer.height(area.width)),
        ])
        .split(area);
    ([chunks[0], chunks[1], chunks[2]], narrow)
}

/// A header's opening spans: the game's `title` badge, and the daily badge
/// when the game is a daily one.
pub(crate) fn header_spans(title: &str, daily: Option<&str>) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(title.to_string(), black_on(COLOR_ACCENT))];
    if let Some(date) = daily {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!(" DAILY {date} UTC "),
            black_on(COLOR_POSITIVE),
        ));
    }
    spans
}

/// Draws a header line in its box, wrapping rather than cutting it.
pub(crate) fn draw_header(f: &mut Frame, spans: Vec<Span>, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// Draws a frame with `draw`, on true black when colours are on (see
/// [`paint_background`]).
pub(crate) fn draw_on_black(f: &mut Frame, use_color: bool, draw: impl FnOnce(&mut Frame)) {
    draw(f);
    if use_color {
        paint_background(f.buffer_mut());
    }
}

/// A status message as lines in `color`, one per line of `msg`.
pub(crate) fn status_lines(msg: &str, color: Color) -> Vec<Line<'_>> {
    msg.lines()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(color))))
        .collect()
}

/// Draws a status message in its box, wrapped to fit.
pub(crate) fn draw_status(f: &mut Frame, area: Rect, msg: &str, color: Color) {
    f.render_widget(
        Paragraph::new(status_lines(msg, color))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Status ")),
        area,
    );
}

#[cfg(test)]
/// What `draw` puts on a `width` by `height` screen.
pub(crate) fn render_buffer(
    width: u16,
    height: u16,
    draw: impl FnOnce(&mut Frame),
) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
    terminal.draw(draw).unwrap();
    terminal.backend().buffer().clone()
}

#[cfg(test)]
/// A screen's text, a line per row.
pub(crate) fn screen_text(buf: &ratatui::buffer::Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Moves a list's selection one row down (or up), staying within its `len` rows.
pub(crate) fn step_selection(state: &mut TableState, len: usize, down: bool) {
    if len == 0 {
        return;
    }
    let i = state.selected().unwrap_or(0);
    state.select(Some(if down {
        (i + 1).min(len - 1)
    } else {
        i.saturating_sub(1)
    }));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPanel {
    Guess,
    Scratch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterAction {
    Guess,
    Help,
    NewGame,
    Daily,
    GiveUp,
    Difficulty,
    Quit,
}

const FOOTER_ACTIONS: [(FooterAction, &str, &str); 7] = [
    (FooterAction::Guess, "[Enter]", "Guess"),
    (FooterAction::Help, "[?]", "Help"),
    (FooterAction::NewGame, "[^N]", "New Game"),
    (FooterAction::Daily, "[^T]", "Daily"),
    (FooterAction::GiveUp, "[^G]", "Give Up"),
    (FooterAction::Difficulty, "[^D]", "Difficulty"),
    (FooterAction::Quit, "[Esc]", "Quit"),
];

struct TuiApp {
    game: GameState,
    use_color: bool,
    input: String,
    focus: FocusPanel,
    scratch_cursor: usize,
    guess_area: Rect,
    scratch_area: Rect,
    status_area: Rect,
    footer: Footer<FooterAction>,
    message: Option<(String, Color)>,
    show_help: bool,
    should_quit: bool,
    table_state: TableState,
}

impl TuiApp {
    fn new(game: GameState) -> Self {
        let mode = game.mode();
        let initial_message = if let Some(date) = game.daily_date() {
            (
                format!(
                    "Daily Challenge ({date} UTC) started!\nDifficulty: {} ({} words).",
                    game.difficulty(),
                    dict::word_count(mode, game.difficulty().scowl_size())
                ),
                COLOR_ACCENT,
            )
        } else {
            let prompt = match mode {
                GameMode::Jotto => "Enter a 5-letter word with no repeating letters.",
                GameMode::Wordle => {
                    "Enter a 5-letter word (6 guesses max, repeating letters allowed)."
                }
            };
            (prompt.to_string(), Color::Gray)
        };

        Self {
            game,
            use_color: supports_color(),
            input: String::new(),
            focus: FocusPanel::Guess,
            scratch_cursor: 0,
            guess_area: Rect::default(),
            scratch_area: Rect::default(),
            status_area: Rect::default(),
            footer: Footer::new(&FOOTER_ACTIONS),
            message: Some(initial_message),
            show_help: false,
            should_quit: false,
            table_state: TableState::default(),
        }
    }

    fn submit_input(&mut self) {
        if self.game.status() != GameStatus::Playing {
            self.input.clear();
            return;
        }

        let trimmed = self.input.trim().to_ascii_lowercase();
        self.input.clear();

        if trimmed.is_empty() {
            return;
        }

        match trimmed.as_str() {
            "help" | ":help" | "?" => {
                self.show_help = true;
                return;
            }
            "giveup" | ":giveup" | "give up" => {
                self.trigger_give_up();
                return;
            }
            "new" | ":new" | "reset" => {
                self.start_new_game(None);
                return;
            }
            "daily" | ":daily" => {
                self.start_daily_game();
                return;
            }
            "exit" | ":exit" | "quit" | ":quit" => {
                self.should_quit = true;
                return;
            }
            _ => {}
        }

        let mode = self.game.mode();
        let prev_len = self.game.history().len();
        match self.game.make_guess(&trimmed) {
            Ok(round) => {
                let is_duplicate = self.game.history().len() == prev_len;
                if is_duplicate {
                    let row_idx = match mode {
                        GameMode::Jotto => prev_len.saturating_sub(round.round),
                        GameMode::Wordle => round.round.saturating_sub(1),
                    };
                    self.table_state.select(Some(row_idx));
                    let dup_msg = match mode {
                        GameMode::Jotto => format!(
                            "Already guessed '{}' in Round {} ({} in common).",
                            round.guess.to_ascii_uppercase(),
                            round.round,
                            plural(round.common, "letter", "letters"),
                        ),
                        GameMode::Wordle => format!(
                            "Already guessed '{}' in Guess {}.",
                            round.guess.to_ascii_uppercase(),
                            round.round
                        ),
                    };
                    self.message = Some((dup_msg, COLOR_WARNING));
                    return;
                }

                let selected_row = match mode {
                    GameMode::Jotto => 0,
                    GameMode::Wordle => self.game.history().len().saturating_sub(1),
                };
                self.table_state.select(Some(selected_row));

                match self.game.status() {
                    GameStatus::Won => {
                        let win_msg = match mode {
                            GameMode::Jotto => format!(
                                "CONGRATULATIONS!!! You guessed {} in {} {}.\nClick [New Game] or press [^N] to play again.",
                                self.game.secret_word().to_ascii_uppercase(),
                                self.game.history().len(),
                                if self.game.history().len() == 1 {
                                    "guess"
                                } else {
                                    "guesses"
                                }
                            ),
                            GameMode::Wordle => {
                                self.message = self.wordle_end_message("Click/[C] Copy");
                                return;
                            }
                        };
                        self.message = Some((win_msg, COLOR_POSITIVE));
                    }
                    GameStatus::Lost => {
                        self.message = self.wordle_end_message("Click/[C] Copy");
                    }
                    GameStatus::Playing | GameStatus::GivenUp => {
                        let status_msg = match mode {
                            GameMode::Jotto => format!(
                                "Round {}: '{}' has {} in common.{}",
                                round.round,
                                round.guess.to_ascii_uppercase(),
                                plural(round.common, "letter", "letters"),
                                if round.common == 0 {
                                    " Its letters are scratched out."
                                } else {
                                    ""
                                }
                            ),
                            GameMode::Wordle => {
                                let remaining =
                                    MAX_WORDLE_GUESSES.saturating_sub(self.game.history().len());
                                format!(
                                    "Guess {}/{}: '{}' ({} {} remaining).",
                                    round.round,
                                    MAX_WORDLE_GUESSES,
                                    round.guess.to_ascii_uppercase(),
                                    remaining,
                                    if remaining == 1 { "guess" } else { "guesses" }
                                )
                            }
                        };
                        self.message = Some((status_msg, COLOR_ACCENT));
                    }
                }
            }
            Err(err) => {
                self.message = Some((err.to_string(), COLOR_WARNING));
            }
        }
    }

    fn start_new_game(&mut self, difficulty: Option<crate::game::Difficulty>) {
        self.game.reset(difficulty);
        self.input.clear();
        self.focus = FocusPanel::Guess;
        self.table_state.select(None);
        self.message = Some((
            format!(
                "New game started!\nDifficulty: {} ({} words).",
                self.game.difficulty(),
                dict::word_count(self.game.mode(), self.game.difficulty().scowl_size())
            ),
            COLOR_ACCENT,
        ));
    }

    fn start_daily_game(&mut self) {
        self.game.start_daily();
        self.input.clear();
        self.focus = FocusPanel::Guess;
        self.table_state.select(None);
        let date_str = self.game.daily_date().unwrap_or("today");
        self.message = Some((
            format!(
                "Daily Challenge ({date_str} UTC) started!\nDifficulty: {} ({} words).",
                self.game.difficulty(),
                dict::word_count(self.game.mode(), self.game.difficulty().scowl_size())
            ),
            COLOR_ACCENT,
        ));
    }

    fn cycle_difficulty(&mut self) {
        let next_diff = self.game.difficulty().next();
        self.start_new_game(Some(next_diff));
    }

    fn trigger_give_up(&mut self) {
        if self.game.status() != GameStatus::Playing {
            return;
        }
        self.game.give_up();
        self.input.clear();
        self.message = Some((
            format!(
                "Your word is: {}\nClick [New Game] or press [^N] to play again.",
                self.game.secret_word().to_ascii_uppercase()
            ),
            COLOR_WARNING,
        ));
    }

    fn toggle_focus(&mut self) {
        if self.game.mode() == GameMode::Wordle {
            self.focus = FocusPanel::Guess;
            return;
        }
        self.focus = match self.focus {
            FocusPanel::Guess => FocusPanel::Scratch,
            FocusPanel::Scratch => FocusPanel::Guess,
        };
    }

    fn scroll_history(&mut self, down: bool) {
        step_selection(&mut self.table_state, self.game.history().len(), down);
    }

    fn copy_wordle_summary(&mut self) {
        if self.game.mode() != GameMode::Wordle
            || !matches!(self.game.status(), GameStatus::Won | GameStatus::Lost)
            || self.game.history().is_empty()
        {
            return;
        }
        let summary = self.game.wordle_share_summary();
        let _ = copy_to_clipboard(&summary);
        self.message = self.wordle_end_message("Copied summary!");
    }

    /// The message for a finished Wordle game, `copy` saying how to copy (or
    /// that it was) its summary.
    fn wordle_end_message(&self, copy: &str) -> Option<(String, Color)> {
        let word = self.game.secret_word().to_ascii_uppercase();
        match self.game.status() {
            GameStatus::Won => Some((
                format!(
                    "You guessed {word} in {}/{MAX_WORDLE_GUESSES}!\n{copy} | [^N] New Game",
                    self.game.history().len()
                ),
                COLOR_POSITIVE,
            )),
            GameStatus::Lost => Some((
                format!("Out of guesses! Word: {word}\n{copy} | [^N] New Game"),
                COLOR_NEGATIVE,
            )),
            _ => self.message.clone(),
        }
    }

    fn execute_footer_action(&mut self, action: FooterAction) {
        match action {
            FooterAction::Guess => {
                self.focus = FocusPanel::Guess;
                self.submit_input();
            }
            FooterAction::Help => {
                self.show_help = !self.show_help;
            }
            FooterAction::NewGame => {
                self.start_new_game(None);
            }
            FooterAction::Daily => {
                self.start_daily_game();
            }
            FooterAction::GiveUp => {
                self.trigger_give_up();
            }
            FooterAction::Difficulty => {
                self.cycle_difficulty();
            }
            FooterAction::Quit => {
                self.should_quit = true;
            }
        }
    }

    fn handle_mouse_click(&mut self, col: u16, row: u16) {
        // Check footer buttons first
        if let Some(action) = self.footer.hit(col, row) {
            self.execute_footer_action(action);
            return;
        }

        // Check Guess box click
        if rect_contains(self.guess_area, col, row) {
            self.focus = FocusPanel::Guess;
            return;
        }

        // In Wordle mode, clicking the Status box when the game is won/lost copies the summary
        if self.game.mode() == GameMode::Wordle && rect_contains(self.status_area, col, row) {
            self.copy_wordle_summary();
            return;
        }

        // Check Scratch box click (only in Jotto mode; does not steal focus from Guess box)
        if self.game.mode() != GameMode::Jotto {
            return;
        }
        let area = self.scratch_area;
        if !rect_contains(area, col, row) {
            return;
        }

        if col <= area.x
            || col >= area.x + area.width.saturating_sub(1)
            || row <= area.y
            || row >= area.y + area.height.saturating_sub(1)
        {
            return;
        }

        let inner_width = area.width.saturating_sub(2);
        let cell_w = letter_cell_width(inner_width);
        let row_width = 13 * cell_w;
        let start_x = area.x + 1 + inner_width.saturating_sub(row_width) / 2;

        if col < start_x || col >= start_x + row_width {
            return;
        }

        let letter_col = ((col - start_x) / cell_w) as usize;
        if letter_col >= 13 {
            return;
        }

        let rel_y = row - area.y;
        let idx = if rel_y == 2 {
            Some(letter_col)
        } else if rel_y == 4 {
            Some(13 + letter_col)
        } else {
            None
        };

        if let Some(i) = idx {
            self.scratch_cursor = i;
            let ch = (b'a' + i as u8) as char;
            self.game.toggle_scratch(ch);
        }
    }
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((triple >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Emit OSC 52 clipboard sequence for modern terminals (iTerm2, Ghostty, Kitty, WezTerm, etc.)
    let b64 = encode_base64(text.as_bytes());
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();

    let commands: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(target_os = "windows") {
        &[("clip", &[])]
    } else {
        &[
            ("termux-clipboard-set", &[]),
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };

    for &(cmd, args) in commands {
        if let Ok(mut child) = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                if stdin.write_all(text.as_bytes()).is_ok() {
                    drop(stdin);
                    if let Ok(status) = child.wait() {
                        if status.success() {
                            return true;
                        }
                    }
                }
            }
        }
    }

    false
}

fn letter_cell_width(inner_width: u16) -> u16 {
    if inner_width >= 13 * 4 {
        4
    } else {
        3
    }
}

pub fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

pub(crate) fn wrapped_line_count(text: &str, max_width: u16) -> u16 {
    let max_w = max_width.max(1) as usize;
    let mut total_lines = 0u16;
    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            total_lines += 1;
            continue;
        }
        let mut current_len = 0usize;
        let mut line_count = 1u16;
        for word in trimmed.split_whitespace() {
            let w_len = word.chars().count();
            if current_len == 0 {
                if w_len > max_w {
                    line_count += (w_len.div_ceil(max_w) - 1) as u16;
                    current_len = w_len % max_w;
                    if current_len == 0 {
                        current_len = max_w;
                    }
                } else {
                    current_len = w_len;
                }
            } else if current_len + 1 + w_len <= max_w {
                current_len += 1 + w_len;
            } else {
                line_count += 1;
                if w_len > max_w {
                    line_count += (w_len.div_ceil(max_w) - 1) as u16;
                    current_len = w_len % max_w;
                    if current_len == 0 {
                        current_len = max_w;
                    }
                } else {
                    current_len = w_len;
                }
            }
        }
        total_lines += line_count;
    }
    total_lines.max(1)
}

/// RAII guard that ensures the terminal is restored on exit or panic unwinding.
pub struct TerminalGuard;

/// Switches the terminal to full-screen raw mode for a TUI. It's restored when the
/// guard drops, and by a panic hook too, so a panic message prints on the normal
/// screen rather than vanishing with the alternate one.
pub fn enter_tui() -> io::Result<(TerminalGuard, Terminal<CrosstermBackend<io::Stdout>>)> {
    // Once a process: `arcade` enters a TUI for every game it runs, and wrapping
    // the hook each time would stack a layer per game.
    static PANIC_HOOK: std::sync::Once = std::sync::Once::new();
    PANIC_HOOK.call_once(|| {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            let _ = disable_raw_mode();
            let _ = execute!(
                io::stdout(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                Show
            );
            original_hook(panic_info);
        }));
    });

    enable_raw_mode()?;
    // Made before anything else can fail, so raw mode never outlives an error. It
    // comes first in the tuple so the terminal drops before it restores the screen.
    let guard = TerminalGuard;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok((guard, Terminal::new(CrosstermBackend::new(stdout))?))
}

/// A palette colour as an ANSI `r;g;b` triple, for plain mode's escape codes.
pub(crate) fn ansi_rgb(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("{r};{g};{b}"),
        other => unreachable!("palette colours are RGB, not {other:?}"),
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            Show
        );
    }
}

/// Runs the full-screen interactive TUI.
pub fn run(game: GameState, autosave: &mut Autosave) -> io::Result<()> {
    let mut app = TuiApp::new(game);
    if let Some(notice) = autosave.notice.take() {
        app.message = Some((notice.text().to_string(), notice_color(&notice)));
    }
    run_screen(&mut app, autosave)
}

/// Colour of the status line telling the player about their save.
pub(crate) fn notice_color(notice: &Notice) -> Color {
    match notice {
        Notice::Resumed(_) => COLOR_ACCENT,
        Notice::Problem(_) => COLOR_WARNING,
    }
}

/// A game's full-screen TUI, as [`run_screen`] runs it.
pub(crate) trait GameScreen {
    /// What the game saves, synced after every event.
    type Saved: serde::Serialize;
    fn draw(&mut self, f: &mut Frame);
    fn saved(&self) -> Self::Saved;
    fn should_quit(&self) -> bool;
    /// Whether the help page is open, which takes every key and click until
    /// it's closed.
    fn show_help(&mut self) -> &mut bool;
    /// A mouse event while help is closed.
    fn on_mouse(&mut self, mouse: MouseEvent);
    /// A key press while help is closed (Ctrl+C has already quit); `Break` quits.
    fn on_key(&mut self, key: KeyEvent, ctrl: bool) -> ControlFlow<()>;
}

/// Runs `app` full screen until it quits, saving each change.
pub(crate) fn run_screen(app: &mut impl GameScreen, autosave: &mut Autosave) -> io::Result<()> {
    let (_guard, mut terminal) = enter_tui()?;
    loop {
        terminal.draw(|f| app.draw(f))?;
        // Every key and click comes back here, so this saves each change.
        autosave.sync(&app.saved());
        if app.should_quit() {
            return Ok(());
        }
        match crate::input::read()? {
            Event::Mouse(mouse) if *app.show_help() => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    *app.show_help() = false;
                }
            }
            Event::Mouse(mouse) => app.on_mouse(mouse),
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('c') {
                    return Ok(());
                }
                if *app.show_help() {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q')
                    ) {
                        *app.show_help() = false;
                    }
                } else if app.on_key(key, ctrl).is_break() {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

impl GameScreen for TuiApp {
    type Saved = crate::game::SavedGame;

    fn draw(&mut self, f: &mut Frame) {
        draw_ui(f, self);
    }

    fn saved(&self) -> Self::Saved {
        self.game.to_saved()
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn show_help(&mut self) -> &mut bool {
        &mut self.show_help
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.handle_mouse_click(mouse.column, mouse.row);
            }
            MouseEventKind::ScrollUp => {
                self.scroll_history(false);
            }
            MouseEventKind::ScrollDown => {
                self.scroll_history(true);
            }
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctrl: bool) -> ControlFlow<()> {
        match key.code {
            KeyCode::Esc => return ControlFlow::Break(()),
            KeyCode::Tab | KeyCode::BackTab => {
                self.toggle_focus();
            }
            KeyCode::Char('?') => {
                self.show_help = true;
            }
            KeyCode::Char('n') if ctrl => {
                self.start_new_game(None);
            }
            KeyCode::Char('t') if ctrl => {
                self.start_daily_game();
            }
            KeyCode::Char('g') if ctrl => {
                self.trigger_give_up();
            }
            KeyCode::Char('d') if ctrl => {
                self.cycle_difficulty();
            }
            KeyCode::Char('c') | KeyCode::Char('C')
                if self.game.mode() == GameMode::Wordle
                    && matches!(self.game.status(), GameStatus::Won | GameStatus::Lost) =>
            {
                self.copy_wordle_summary();
            }
            _ => match self.focus {
                FocusPanel::Guess => match key.code {
                    KeyCode::Enter => {
                        self.submit_input();
                    }
                    KeyCode::Backspace => {
                        if self.game.status() == GameStatus::Playing {
                            self.input.pop();
                        }
                    }
                    KeyCode::Up => {
                        self.scroll_history(false);
                    }
                    KeyCode::Down => {
                        self.scroll_history(true);
                    }
                    KeyCode::Char(c)
                        if c.is_ascii_alphabetic()
                            && self.game.status() == GameStatus::Playing
                            && self.input.len() < 12 =>
                    {
                        self.input.push(c.to_ascii_lowercase());
                    }
                    _ => {}
                },
                FocusPanel::Scratch => match key.code {
                    KeyCode::Left => {
                        self.scratch_cursor = (self.scratch_cursor + 25) % 26;
                    }
                    KeyCode::Right => {
                        self.scratch_cursor = (self.scratch_cursor + 1) % 26;
                    }
                    KeyCode::Up | KeyCode::Down => {
                        self.scratch_cursor = (self.scratch_cursor + 13) % 26;
                    }
                    KeyCode::Char(' ') | KeyCode::Enter => {
                        let ch = (b'a' + self.scratch_cursor as u8) as char;
                        self.game.toggle_scratch(ch);
                    }
                    KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                        let lower = c.to_ascii_lowercase();
                        self.scratch_cursor = (lower as u8 - b'a') as usize;
                        self.game.toggle_scratch(lower);
                    }
                    _ => {}
                },
            },
        }
        ControlFlow::Continue(())
    }
}

/// Draws a frame, on true black when colours are on (see [`paint_background`]).
fn draw_ui(f: &mut Frame, app: &mut TuiApp) {
    draw_on_black(f, app.use_color, |f| draw_screen(f, app));
}

fn draw_screen(f: &mut Frame, app: &mut TuiApp) {
    let mode = app.game.mode();
    let mode_title = format!(" {} ", mode.title());

    if draw_too_narrow(f, &mode_title, mode.command_name()) {
        app.guess_area = Rect::default();
        app.scratch_area = Rect::default();
        app.status_area = Rect::default();
        app.footer.clear();
        return;
    }

    let (chunks, narrow) = screen_areas(f.area(), &app.footer, 14);

    // Header
    let pool_count = dict::word_count(mode, app.game.difficulty().scowl_size());
    let mut header_spans = header_spans(&mode_title, app.game.daily_date());
    let counter_label = match mode {
        GameMode::Jotto => "  |  Rounds: ",
        GameMode::Wordle => "  |  Guesses: ",
    };
    let counter_val = match mode {
        GameMode::Jotto => app.game.history().len().to_string(),
        GameMode::Wordle => format!("{}/{}", app.game.history().len(), MAX_WORDLE_GUESSES),
    };
    header_spans.extend([
        Span::raw("  Difficulty: "),
        Span::styled(
            format!("{} ({} words)", app.game.difficulty(), pool_count),
            bold(COLOR_ACCENT),
        ),
        Span::raw(counter_label),
        Span::styled(counter_val, bold(Color::White)),
    ]);
    draw_header(f, header_spans, chunks[0]);

    // Main split body (Left: Input + Used Letters [+ Scratch] + Status, Right: History / Wordle Board).
    // On a narrow screen the left column's boxes go on top, at their own heights,
    // and the history or board fills the rest below.
    let body_chunks = if narrow {
        let stack_h = match mode {
            GameMode::Jotto => 3 + 7 + 7 + 6,
            GameMode::Wordle => 3 + 7 + 6,
        };
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(stack_h), Constraint::Min(6)])
            .split(chunks[1])
    } else {
        let body_constraints = if f.area().width < 86 {
            [Constraint::Length(41), Constraint::Min(24)]
        } else {
            [Constraint::Percentage(48), Constraint::Percentage(52)]
        };
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints(body_constraints)
            .split(chunks[1])
    };

    let (guess_rect, used_rect, scratch_rect, status_rect) = match mode {
        GameMode::Jotto => {
            let lc = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(7),
                    Constraint::Length(7),
                    Constraint::Min(4),
                ])
                .split(body_chunks[0]);
            (lc[0], lc[1], Some(lc[2]), lc[3])
        }
        GameMode::Wordle => {
            let used_h = if body_chunks[0].height < 21 { 5 } else { 7 };
            let lc = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Length(used_h),
                    Constraint::Min(4),
                ])
                .split(body_chunks[0]);
            (lc[0], lc[1], None, lc[2])
        }
    };

    app.guess_area = guess_rect;
    app.scratch_area = scratch_rect.unwrap_or_default();
    app.status_area = status_rect;

    // Guess Input Box
    let input_title = match app.game.status() {
        GameStatus::Playing => " Guess (5 letters) ",
        GameStatus::Won => " Game Won! Press [^N] for New Game ",
        GameStatus::Lost => " Out of Guesses! Press [^N] for New Game ",
        GameStatus::GivenUp => " Game Over! Press [^N] for New Game ",
    };
    let guess_border_style = if app.focus == FocusPanel::Guess {
        Style::default().fg(COLOR_ACCENT)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let cursor_suffix =
        if app.focus == FocusPanel::Guess && app.game.status() == GameStatus::Playing {
            "_"
        } else {
            ""
        };
    let input_widget = Paragraph::new(format!(
        "{}{}",
        app.input.to_ascii_uppercase(),
        cursor_suffix
    ))
    .style(bold(Color::White))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(input_title)
            .border_style(guess_border_style),
    );
    f.render_widget(input_widget, guess_rect);

    let used_cell_w = letter_cell_width(used_rect.width.saturating_sub(2));
    let used_pad = if used_cell_w == 4 { " " } else { "" };

    // Used Letters Box (Jotto: binary used/unused; Wordle: Correct/Present/Absent with gray absent letters)
    let mut used_row1 = Vec::new();
    let mut used_row2 = Vec::new();
    match mode {
        GameMode::Jotto => {
            let used = app.game.used_letters();
            for (i, &is_used) in used.iter().enumerate() {
                let ch = (b'A' + i as u8) as char;
                let span = if is_used {
                    Span::styled(format!("[{ch}]{used_pad}"), black_on(COLOR_ACCENT))
                } else {
                    Span::styled(
                        format!(" {ch} {used_pad}"),
                        Style::default().fg(Color::DarkGray),
                    )
                };
                if i < 13 {
                    used_row1.push(span);
                } else {
                    used_row2.push(span);
                }
            }
        }
        GameMode::Wordle => {
            let states = app.game.letter_states();
            for (i, &state) in states.iter().enumerate() {
                let ch = (b'A' + i as u8) as char;
                let span = match state {
                    Some(LetterState::Correct) => Span::styled(
                        if app.use_color {
                            format!(" {ch} {used_pad}")
                        } else {
                            format!("+{ch}+{used_pad}")
                        },
                        black_on(COLOR_POSITIVE),
                    ),
                    Some(LetterState::Present) => Span::styled(
                        if app.use_color {
                            format!(" {ch} {used_pad}")
                        } else {
                            format!("?{ch}?{used_pad}")
                        },
                        black_on(COLOR_WARNING),
                    ),
                    Some(LetterState::Absent) => Span::styled(
                        if app.use_color {
                            format!(" {ch} {used_pad}")
                        } else {
                            format!("~{ch}~{used_pad}")
                        },
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    None => Span::styled(
                        format!(" {ch} {used_pad}"),
                        Style::default().fg(Color::Gray),
                    ),
                };
                if i < 13 {
                    used_row1.push(span);
                } else {
                    used_row2.push(span);
                }
            }
        }
    }
    let used_lines = if used_rect.height < 7 {
        vec![Line::from(used_row1), Line::from(""), Line::from(used_row2)]
    } else {
        vec![
            Line::from(""),
            Line::from(used_row1),
            Line::from(""),
            Line::from(used_row2),
        ]
    };
    let used_title = match (mode, app.use_color) {
        (GameMode::Jotto, _) | (GameMode::Wordle, true) => " Used Letters ",
        (GameMode::Wordle, false) => " Used Letters (+exact ?present ~absent) ",
    };
    let used_widget = Paragraph::new(used_lines)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).title(used_title));
    f.render_widget(used_widget, used_rect);

    // Scratch Box (Jotto only: 3 colorblind-safe states: Default -> ScratchedOut -> Positive)
    if let Some(s_rect) = scratch_rect {
        let scratch_cell_w = letter_cell_width(s_rect.width.saturating_sub(2));
        let scratch_pad = if scratch_cell_w == 4 { " " } else { "" };
        let scratched = app.game.scratched_letters();
        let mut scratch_row1 = Vec::new();
        let mut scratch_row2 = Vec::new();
        for (i, &state) in scratched.iter().enumerate() {
            let ch = (b'A' + i as u8) as char;
            let is_cursor = app.focus == FocusPanel::Scratch && app.scratch_cursor == i;
            let (label, mut style) = match state {
                ScratchState::Default => {
                    if is_cursor {
                        (
                            format!("[{ch}]{scratch_pad}"),
                            Style::default()
                                .fg(COLOR_ACCENT)
                                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                        )
                    } else {
                        (
                            format!(" {ch} {scratch_pad}"),
                            Style::default().fg(Color::Gray),
                        )
                    }
                }
                ScratchState::ScratchedOut => (
                    format!("~{ch}~{scratch_pad}"),
                    Style::default()
                        .fg(Color::Black)
                        .bg(COLOR_NEGATIVE)
                        .add_modifier(Modifier::BOLD | Modifier::CROSSED_OUT),
                ),
                ScratchState::Positive => {
                    (format!("+{ch}+{scratch_pad}"), black_on(COLOR_POSITIVE))
                }
            };
            if is_cursor {
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            let span = Span::styled(label, style);
            if i < 13 {
                scratch_row1.push(span);
            } else {
                scratch_row2.push(span);
            }
        }
        let scratch_lines = vec![
            Line::from(""),
            Line::from(scratch_row1),
            Line::from(""),
            Line::from(scratch_row2),
        ];
        let scratch_border_style = if app.focus == FocusPanel::Scratch {
            Style::default().fg(COLOR_ACCENT)
        } else {
            Style::default()
        };
        let scratch_title = if s_rect.width >= 52 {
            " Scratch (Click or [Tab] + type/Space to toggle) "
        } else {
            " Scratch (Click or [Tab] to toggle) "
        };
        let scratch_widget = Paragraph::new(scratch_lines)
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(scratch_title)
                    .border_style(scratch_border_style),
            );
        f.render_widget(scratch_widget, s_rect);
    }

    // Status / Message Box
    let (msg_text, msg_color) = app
        .message
        .as_ref()
        .map(|(m, c)| (m.as_str(), *c))
        .unwrap_or(("", Color::White));
    let status_lines = status_lines(msg_text, msg_color);

    let show_wordle_summary = mode == GameMode::Wordle
        && matches!(app.game.status(), GameStatus::Won | GameStatus::Lost)
        && !app.game.history().is_empty();

    let status_title = if show_wordle_summary {
        " Status (Click or [C] to Copy) "
    } else {
        " Status "
    };
    let status_block = Block::default()
        .borders(Borders::ALL)
        .title(status_title)
        .border_style(Style::default().fg(Color::White));

    if show_wordle_summary {
        let status_inner = status_block.inner(status_rect);
        f.render_widget(status_block, status_rect);

        let mut summary_lines = Vec::with_capacity(app.game.history().len() + 1);
        for r in app.game.history() {
            let mut spans = Vec::with_capacity(9);
            for (idx, &state) in r.letter_states.iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::raw(" "));
                }
                let tile = match state {
                    LetterState::Correct => Span::styled(
                        if app.use_color { "   " } else { " + " },
                        black_on(COLOR_POSITIVE),
                    ),
                    LetterState::Present => Span::styled(
                        if app.use_color { "   " } else { " ? " },
                        black_on(COLOR_WARNING),
                    ),
                    LetterState::Absent => Span::styled(
                        if app.use_color { "   " } else { " ~ " },
                        if app.use_color {
                            Style::default().fg(Color::Gray).bg(Color::DarkGray)
                        } else {
                            Style::default()
                                .fg(Color::Gray)
                                .bg(Color::DarkGray)
                                .add_modifier(Modifier::CROSSED_OUT)
                        },
                    ),
                };
                spans.push(tile);
            }
            summary_lines.push(Line::from(spans));
        }

        let msg_h = wrapped_line_count(msg_text, status_inner.width);
        let history_h = app.game.history().len() as u16;

        if status_inner.height >= msg_h + history_h {
            let status_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(msg_h), Constraint::Min(1)])
                .split(status_inner);

            let msg_widget = Paragraph::new(status_lines).wrap(Wrap { trim: true });
            f.render_widget(msg_widget, status_chunks[0]);

            if status_chunks[1].height > history_h {
                summary_lines.insert(0, Line::from(""));
            }
            let summary_widget = Paragraph::new(summary_lines).alignment(Alignment::Center);
            f.render_widget(summary_widget, status_chunks[1]);
        } else {
            let status_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(15), Constraint::Length(21)])
                .split(status_inner);

            let msg_widget = Paragraph::new(status_lines).wrap(Wrap { trim: true });
            f.render_widget(msg_widget, status_chunks[0]);

            let summary_widget = Paragraph::new(summary_lines).alignment(Alignment::Center);
            f.render_widget(summary_widget, status_chunks[1]);
        }
    } else {
        let status_widget = Paragraph::new(status_lines)
            .wrap(Wrap { trim: true })
            .block(status_block);
        f.render_widget(status_widget, status_rect);
    }

    // Right Panel: History Table (Jotto) or 6-Guess Board (Wordle)
    match mode {
        GameMode::Jotto => {
            let header_cells = ["Round", "Guess", "Common"]
                .iter()
                .map(|h| Cell::from(*h).style(bold(COLOR_ACCENT)));
            let table_header = Row::new(header_cells).height(1).bottom_margin(1);

            let rows = app.game.history().iter().rev().map(|r| {
                let common_style = if r.common == 5 {
                    bold(COLOR_POSITIVE)
                } else {
                    Style::default().fg(Color::White)
                };
                Row::new(vec![
                    Cell::from(r.round.to_string()),
                    Cell::from(r.guess.to_ascii_uppercase()),
                    Cell::from(r.common.to_string()).style(common_style),
                ])
            });

            let history_table = Table::new(
                rows,
                [
                    Constraint::Percentage(25),
                    Constraint::Percentage(45),
                    Constraint::Percentage(30),
                ],
            )
            .header(table_header)
            .block(Block::default().borders(Borders::ALL).title(" History "))
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

            f.render_stateful_widget(history_table, body_chunks[1], &mut app.table_state);
        }
        GameMode::Wordle => {
            let row_margin = if body_chunks[1].height >= 16 { 1 } else { 0 };
            let header_cells = ["Guess", "Board"]
                .iter()
                .map(|h| Cell::from(*h).style(bold(COLOR_ACCENT)));
            let table_header = Row::new(header_cells).height(1).bottom_margin(row_margin);

            let mut rows = Vec::with_capacity(MAX_WORDLE_GUESSES);
            for slot in 0..MAX_WORDLE_GUESSES {
                if let Some(r) = app.game.history().get(slot) {
                    let mut spans = Vec::with_capacity(9);
                    for (idx, (ch, &state)) in r
                        .guess
                        .chars()
                        .map(|c| c.to_ascii_uppercase())
                        .zip(r.letter_states.iter())
                        .enumerate()
                    {
                        if idx > 0 {
                            spans.push(Span::raw(" "));
                        }
                        let span = match state {
                            LetterState::Correct => Span::styled(
                                if app.use_color {
                                    format!(" {ch} ")
                                } else {
                                    format!("+{ch}+")
                                },
                                black_on(COLOR_POSITIVE),
                            ),
                            LetterState::Present => Span::styled(
                                if app.use_color {
                                    format!(" {ch} ")
                                } else {
                                    format!("?{ch}?")
                                },
                                black_on(COLOR_WARNING),
                            ),
                            LetterState::Absent => Span::styled(
                                if app.use_color {
                                    format!(" {ch} ")
                                } else {
                                    format!("~{ch}~")
                                },
                                Style::default()
                                    .fg(Color::DarkGray)
                                    .add_modifier(Modifier::CROSSED_OUT),
                            ),
                        };
                        spans.push(span);
                    }
                    rows.push(
                        Row::new(vec![
                            Cell::from(format!("{}/{}", r.round, MAX_WORDLE_GUESSES)),
                            Cell::from(Line::from(spans)),
                        ])
                        .bottom_margin(row_margin),
                    );
                } else {
                    rows.push(
                        Row::new(vec![
                            Cell::from(format!("{}/{}", slot + 1, MAX_WORDLE_GUESSES))
                                .style(Style::default().fg(Color::DarkGray)),
                            Cell::from(Line::from(Span::styled(
                                " ·   ·   ·   ·   · ",
                                Style::default().fg(Color::DarkGray),
                            ))),
                        ])
                        .bottom_margin(row_margin),
                    );
                }
            }

            let board_table = Table::new(
                rows,
                [Constraint::Percentage(25), Constraint::Percentage(75)],
            )
            .header(table_header)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Guesses (6 max) "),
            )
            .row_highlight_style(Style::default().add_modifier(Modifier::BOLD));

            f.render_stateful_widget(board_table, body_chunks[1], &mut app.table_state);
        }
    }

    // Clickable Footer Buttons, in as many rows as the width needs.
    app.footer.draw(f, chunks[2]);

    // Help & Credits Modal
    if app.show_help {
        draw_help(f, mode, app.use_color);
    }
}

/// The help page: rules (and Wordle's colours), controls, levels with their word
/// counts, and credits.
fn draw_help(f: &mut Frame, mode: GameMode, use_color: bool) {
    use crate::game::Difficulty as D;
    let tile = |plain: &'static str, style: Style, what: &'static str| {
        HelpLine::bullet(vec![
            Span::styled(if use_color { " X " } else { plain }, style),
            Span::raw(format!(" {what}")),
        ])
    };
    let (tagline, rules, controls) = match mode {
        GameMode::Jotto => (
            "guess the five-letter word, unlimited guesses",
            vec![
                HelpLine::bullet(
                    "The secret word has 5 different letters and isn't a proper noun.",
                ),
                HelpLine::bullet(
                    "Each guess scores how many letters it shares with it, in any spot.",
                ),
                HelpLine::bullet("Mark letters in the scratch pad as you rule them in or out."),
            ],
            vec![
                HelpLine::Keys(vec![
                    ("Enter", "guess"),
                    ("Tab", "scratch pad"),
                    ("click", "mark a letter"),
                ]),
                HelpLine::Keys(vec![("↑↓", "scroll"), ("?", "help"), ("Esc", "quit")]),
            ],
        ),
        GameMode::Wordle => (
            "find the five-letter word in six guesses",
            vec![
                HelpLine::bullet("Letters may repeat; the word isn't a proper noun or a plural."),
                tile("+X+", black_on(COLOR_POSITIVE), "in the word, in this spot"),
                tile(
                    "?X?",
                    black_on(COLOR_WARNING),
                    "in the word, in another spot",
                ),
                tile(
                    "~X~",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                    "not in the word",
                ),
            ],
            vec![HelpLine::Keys(vec![
                ("Enter", "guess"),
                ("↑↓", "scroll"),
                ("?", "help"),
                ("Esc", "quit"),
            ])],
        ),
    };
    let mut controls = controls;
    controls.push(HelpLine::Keys(vec![
        ("^N", "new"),
        ("^T", "daily"),
        ("^G", "give up"),
        ("^D", "difficulty"),
    ]));
    let levels = [
        (D::Easy, "Easy (35)", "recommended small dictionary"),
        (D::Medium, "Medium (50)", "recommended medium dictionary"),
        (D::Hard, "Hard (70)", "recommended large dictionary"),
        (
            D::Scrabble,
            "Scrabble (80)",
            "strange and unusual word-game words",
        ),
        (
            D::Insane,
            "Insane (95)",
            "every valid 5-letter English word",
        ),
    ]
    .into_iter()
    .map(|(d, name, desc)| HelpLine::Row {
        name: name.to_string(),
        desc: desc.to_string(),
        note: format!(
            "{} words",
            crate::text::grouped(dict::word_count(mode, d.scowl_size()))
        ),
    })
    .collect();
    let mut credits = vec![
        HelpLine::author(),
        HelpLine::Credit("Words", "SCOWL (http://wordlist.aspell.net/)".to_string()),
    ];
    if mode == GameMode::Wordle {
        credits.push(HelpLine::Credit(
            "Wordle",
            "by Josh Wardle: https://www.nytimes.com/games/wordle".to_string(),
        ));
    }
    let page = HelpPage {
        game: match mode {
            GameMode::Jotto => "Jotto",
            GameMode::Wordle => "Wordle",
        },
        tagline: tagline.to_string(),
        sections: vec![
            HelpSection {
                title: "Rules",
                lines: rules,
            },
            HelpSection {
                title: "Controls",
                lines: controls,
            },
            HelpSection {
                title: "Difficulty (SCOWL size) · daily: Scrabble",
                lines: levels,
            },
            HelpSection {
                title: "Credits",
                lines: credits,
            },
        ],
    };
    draw_help_page(f, &page);
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(app: &mut TuiApp, width: u16, height: u16) -> String {
        screen_text(&render_buffer(width, height, |f| draw_ui(f, app)))
    }

    #[test]
    fn narrow_screens_stack_top_to_bottom() {
        for (mode, board) in [(GameMode::Jotto, "History"), (GameMode::Wordle, "Board")] {
            let game =
                GameState::new_with_mode(mode, crate::game::Difficulty::Scrabble, None).unwrap();
            let mut app = TuiApp::new(game);
            let screen = render(&mut app, 44, 50);
            if std::env::var_os("JOTTO_SHOW_TUI").is_some() {
                println!("{screen}");
            }
            assert!(
                !screen.contains("Too Narrow"),
                "{mode:?} refused a phone-sized screen"
            );
            // The guess box sits above the history or board, not beside it.
            let row = |text: &str| screen.lines().position(|l| l.contains(text));
            let guess = row("Guess").expect("no guess box");
            let below = row(board).unwrap_or_else(|| panic!("{mode:?}: no {board}"));
            assert!(
                guess < below,
                "{mode:?}: the {board} isn't below the guess box"
            );
            // Every footer button is on screen, label whole.
            for (_, key, label) in FOOTER_ACTIONS {
                assert!(
                    screen.contains(&format!("{key} {label}")),
                    "{mode:?}: '{label}' is cut"
                );
            }
        }
    }

    #[test]
    fn help_fits_a_small_terminal_and_spreads_on_a_wide_one() {
        for (mode, extra) in [
            (GameMode::Jotto, "scratch pad"),
            (GameMode::Wordle, "Josh Wardle"),
        ] {
            let game =
                GameState::new_with_mode(mode, crate::game::Difficulty::Scrabble, None).unwrap();
            let mut app = TuiApp::new(game);
            app.show_help = true;
            for (w, h) in [(80, 24), (130, 34)] {
                let screen = render(&mut app, w, h);
                if std::env::var_os("JOTTO_SHOW_TUI").is_some() {
                    println!("{screen}");
                }
                for text in [
                    "How to Play",
                    "Controls",
                    "Insane (95)",
                    "SCOWL",
                    extra,
                    "to close",
                ] {
                    assert!(screen.contains(text), "{mode:?} at {w}x{h}: no '{text}'");
                }
            }
        }
    }
}
