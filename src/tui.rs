use std::io::{self, stdout};
use std::panic;

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

use crate::dict;
use crate::game::{
    supports_color, GameMode, GameState, GameStatus, LetterState, ScratchState, MAX_WORDLE_GUESSES,
};
use crate::save::{Autosave, Notice};

/// Okabe-Ito colorblind-safe palette
pub(crate) const COLOR_ACCENT: Color = Color::Rgb(86, 180, 233); // Sky Blue (#56B4E9)
pub(crate) const COLOR_POSITIVE: Color = Color::Rgb(0, 158, 115); // Bluish-Green (#009E73)
pub(crate) const COLOR_NEGATIVE: Color = Color::Rgb(213, 94, 0); // Vermilion / Orange-Red (#D55E00)
pub(crate) const COLOR_WARNING: Color = Color::Rgb(230, 159, 0); // Orange (#E69F00)

/// Minimum recommended terminal width in columns.
pub(crate) const MIN_RECOMMENDED_WIDTH: u16 = 70;

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
    footer_buttons: [Rect; 7],
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
            footer_buttons: [Rect::default(); 7],
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
                            "Already guessed '{}' in Round {} ({} {} in common).",
                            round.guess.to_ascii_uppercase(),
                            round.round,
                            round.common,
                            if round.common == 1 {
                                "letter"
                            } else {
                                "letters"
                            }
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
                            GameMode::Wordle => format!(
                                "You guessed {} in {}/{}!\nClick/[C] Copy | [^N] New Game",
                                self.game.secret_word().to_ascii_uppercase(),
                                self.game.history().len(),
                                MAX_WORDLE_GUESSES
                            ),
                        };
                        self.message = Some((win_msg, COLOR_POSITIVE));
                    }
                    GameStatus::Lost => {
                        self.message = Some((
                            format!(
                                "Out of guesses! Word: {}\nClick/[C] Copy | [^N] New Game",
                                self.game.secret_word().to_ascii_uppercase()
                            ),
                            COLOR_NEGATIVE,
                        ));
                    }
                    GameStatus::Playing | GameStatus::GivenUp => {
                        let status_msg = match mode {
                            GameMode::Jotto => format!(
                                "Round {}: '{}' has {} {} in common.",
                                round.round,
                                round.guess.to_ascii_uppercase(),
                                round.common,
                                if round.common == 1 {
                                    "letter"
                                } else {
                                    "letters"
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

    fn scroll_history_up(&mut self) {
        let len = self.game.history().len();
        if len > 0 {
            let i = self.table_state.selected().unwrap_or(0);
            self.table_state.select(Some(i.saturating_sub(1)));
        }
    }

    fn scroll_history_down(&mut self) {
        let len = self.game.history().len();
        if len > 0 {
            let i = self.table_state.selected().unwrap_or(0);
            self.table_state.select(Some((i + 1).min(len - 1)));
        }
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
        let feedback = "Copied summary! | [^N] New Game";
        match self.game.status() {
            GameStatus::Won => {
                self.message = Some((
                    format!(
                        "You guessed {} in {}/{}!\n{feedback}",
                        self.game.secret_word().to_ascii_uppercase(),
                        self.game.history().len(),
                        MAX_WORDLE_GUESSES
                    ),
                    COLOR_POSITIVE,
                ));
            }
            GameStatus::Lost => {
                self.message = Some((
                    format!(
                        "Out of guesses! Word: {}\n{feedback}",
                        self.game.secret_word().to_ascii_uppercase()
                    ),
                    COLOR_NEGATIVE,
                ));
            }
            _ => {}
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
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Check footer buttons first
        for (i, &rect) in self.footer_buttons.iter().enumerate() {
            if rect_contains(rect, col, row) {
                let action = FOOTER_ACTIONS[i].0;
                self.execute_footer_action(action);
                return;
            }
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

pub(crate) fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn wrapped_line_count(text: &str, max_width: u16) -> u16 {
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
pub(crate) struct TerminalGuard;

/// Switches the terminal to full-screen raw mode for a TUI. It's restored when the
/// guard drops, and by a panic hook too, so a panic message prints on the normal
/// screen rather than vanishing with the alternate one.
pub(crate) fn enter_tui() -> io::Result<(TerminalGuard, Terminal<CrosstermBackend<io::Stdout>>)> {
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
    let (_guard, mut terminal) = enter_tui()?;
    run_event_loop(&mut terminal, &mut app, autosave)
}

/// Colour of the status line telling the player about their save.
pub(crate) fn notice_color(notice: &Notice) -> Color {
    match notice {
        Notice::Resumed(_) => COLOR_ACCENT,
        Notice::Problem(_) => COLOR_WARNING,
    }
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
    autosave: &mut Autosave,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw_ui(f, app))?;
        // Every key and click comes back here, so this saves each change.
        autosave.sync(&app.game.to_saved());

        if app.should_quit {
            return Ok(());
        }

        match event::read()? {
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    app.handle_mouse_click(mouse.column, mouse.row);
                }
                MouseEventKind::ScrollUp if !app.show_help => {
                    app.scroll_history_up();
                }
                MouseEventKind::ScrollDown if !app.show_help => {
                    app.scroll_history_down();
                }
                _ => {}
            },
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    return Ok(());
                }

                if app.show_help {
                    match key.code {
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q') => {
                            app.show_help = false;
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Tab | KeyCode::BackTab => {
                        app.toggle_focus();
                    }
                    KeyCode::Char('?') => {
                        app.show_help = true;
                    }
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.start_new_game(None);
                    }
                    KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.start_daily_game();
                    }
                    KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.trigger_give_up();
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.cycle_difficulty();
                    }
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if app.game.mode() == GameMode::Wordle
                            && matches!(app.game.status(), GameStatus::Won | GameStatus::Lost) =>
                    {
                        app.copy_wordle_summary();
                    }
                    _ => match app.focus {
                        FocusPanel::Guess => match key.code {
                            KeyCode::Enter => {
                                app.submit_input();
                            }
                            KeyCode::Backspace => {
                                if app.game.status() == GameStatus::Playing {
                                    app.input.pop();
                                }
                            }
                            KeyCode::Up => {
                                app.scroll_history_up();
                            }
                            KeyCode::Down => {
                                app.scroll_history_down();
                            }
                            KeyCode::Char(c)
                                if c.is_ascii_alphabetic()
                                    && app.game.status() == GameStatus::Playing
                                    && app.input.len() < 12 =>
                            {
                                app.input.push(c.to_ascii_lowercase());
                            }
                            _ => {}
                        },
                        FocusPanel::Scratch => match key.code {
                            KeyCode::Left => {
                                app.scratch_cursor = (app.scratch_cursor + 25) % 26;
                            }
                            KeyCode::Right => {
                                app.scratch_cursor = (app.scratch_cursor + 1) % 26;
                            }
                            KeyCode::Up | KeyCode::Down => {
                                app.scratch_cursor = (app.scratch_cursor + 13) % 26;
                            }
                            KeyCode::Char(' ') | KeyCode::Enter => {
                                let ch = (b'a' + app.scratch_cursor as u8) as char;
                                app.game.toggle_scratch(ch);
                            }
                            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                                let lower = c.to_ascii_lowercase();
                                app.scratch_cursor = (lower as u8 - b'a') as usize;
                                app.game.toggle_scratch(lower);
                            }
                            _ => {}
                        },
                    },
                }
            }
            _ => {}
        }
    }
}

fn draw_ui(f: &mut Frame, app: &mut TuiApp) {
    let mode = app.game.mode();
    let mode_title = format!(" {} ", mode.title());

    if f.area().width < MIN_RECOMMENDED_WIDTH {
        app.guess_area = Rect::default();
        app.scratch_area = Rect::default();
        app.status_area = Rect::default();
        app.footer_buttons = [Rect::default(); 7];

        let warning_lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "Terminal Width Too Narrow",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "Current width: {} columns (minimum recommended: {} columns).",
                f.area().width,
                MIN_RECOMMENDED_WIDTH
            )),
            Line::from(format!(
                "Please widen your terminal window, or run `{} --plain`.",
                mode.command_name()
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press [Esc] or [Ctrl+C] to quit.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let warning = Paragraph::new(warning_lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(mode_title)
                    .border_style(Style::default().fg(COLOR_WARNING)),
            );
        f.render_widget(warning, f.area());
        return;
    }

    let single_row_constraints = [Constraint::Ratio(1, 7); 7];
    let preview_single_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(single_row_constraints)
        .split(f.area());

    let single_row_text_wraps =
        FOOTER_ACTIONS
            .iter()
            .enumerate()
            .any(|(i, &(_, key_hint, label))| {
                let inner_w = preview_single_row[i].width.saturating_sub(2);
                (key_hint.len() + 1 + label.len()) as u16 > inner_w
            });

    let stack_needs_wrap = FOOTER_ACTIONS
        .iter()
        .enumerate()
        .any(|(i, &(_, key_hint, label))| {
            let inner_w = preview_single_row[i].width.saturating_sub(2);
            key_hint.len().max(label.len()) as u16 > inner_w
        });

    let (footer_height, footer_text_wraps) = if !stack_needs_wrap {
        let h = if single_row_text_wraps { 4 } else { 3 };
        (h, single_row_text_wraps)
    } else {
        let preview_top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 4); 4])
            .split(f.area());
        let preview_bot = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 3); 3])
            .split(f.area());
        let stacked_text_wraps =
            FOOTER_ACTIONS
                .iter()
                .enumerate()
                .any(|(i, &(_, key_hint, label))| {
                    let area = if i < 4 {
                        preview_top[i]
                    } else {
                        preview_bot[i - 4]
                    };
                    let inner_w = area.width.saturating_sub(2);
                    (key_hint.len() + 1 + label.len()) as u16 > inner_w
                });
        let row_h = if stacked_text_wraps { 4 } else { 3 };
        (row_h * 2, stacked_text_wraps)
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(footer_height),
        ])
        .split(f.area());

    // Header
    let pool_count = dict::word_count(mode, app.game.difficulty().scowl_size());
    let mut header_spans = vec![Span::styled(
        mode_title,
        Style::default()
            .fg(Color::Black)
            .bg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(date) = app.game.daily_date() {
        header_spans.push(Span::raw(" "));
        header_spans.push(Span::styled(
            format!(" DAILY {date} UTC "),
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_POSITIVE)
                .add_modifier(Modifier::BOLD),
        ));
    }
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
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(counter_label),
        Span::styled(
            counter_val,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let header =
        Paragraph::new(Line::from(header_spans)).block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // Main split body (Left: Input + Used Letters [+ Scratch] + Status, Right: History / Wordle Board)
    let body_constraints = if f.area().width < 86 {
        [Constraint::Length(41), Constraint::Min(24)]
    } else {
        [Constraint::Percentage(48), Constraint::Percentage(52)]
    };
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(body_constraints)
        .split(chunks[1]);

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
    .style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )
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
                    Span::styled(
                        format!("[{ch}]{used_pad}"),
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    )
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
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_POSITIVE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Some(LetterState::Present) => Span::styled(
                        if app.use_color {
                            format!(" {ch} {used_pad}")
                        } else {
                            format!("?{ch}?{used_pad}")
                        },
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_WARNING)
                            .add_modifier(Modifier::BOLD),
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
                ScratchState::Positive => (
                    format!("+{ch}+{scratch_pad}"),
                    Style::default()
                        .fg(Color::Black)
                        .bg(COLOR_POSITIVE)
                        .add_modifier(Modifier::BOLD),
                ),
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
    let status_lines: Vec<Line> = msg_text
        .lines()
        .map(|line| Line::from(Span::styled(line, Style::default().fg(msg_color))))
        .collect();

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
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_POSITIVE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    LetterState::Present => Span::styled(
                        if app.use_color { "   " } else { " ? " },
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_WARNING)
                            .add_modifier(Modifier::BOLD),
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
            let header_cells = ["Round", "Guess", "Common"].iter().map(|h| {
                Cell::from(*h).style(
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
            });
            let table_header = Row::new(header_cells).height(1).bottom_margin(1);

            let rows = app.game.history().iter().rev().map(|r| {
                let common_style = if r.common == 5 {
                    Style::default()
                        .fg(COLOR_POSITIVE)
                        .add_modifier(Modifier::BOLD)
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
            let header_cells = ["Guess", "Board"].iter().map(|h| {
                Cell::from(*h).style(
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )
            });
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
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(COLOR_POSITIVE)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            LetterState::Present => Span::styled(
                                if app.use_color {
                                    format!(" {ch} ")
                                } else {
                                    format!("?{ch}?")
                                },
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(COLOR_WARNING)
                                    .add_modifier(Modifier::BOLD),
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

    // Clickable Footer Buttons
    let button_areas: [Rect; 7] = if !stack_needs_wrap {
        let row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(single_row_constraints)
            .split(chunks[2]);
        std::array::from_fn(|i| row[i])
    } else {
        let row_h = footer_height / 2;
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(row_h), Constraint::Length(row_h)])
            .split(chunks[2]);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 4); 4])
            .split(rows[0]);
        let bot = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 3); 3])
            .split(rows[1]);
        std::array::from_fn(|i| if i < 4 { top[i] } else { bot[i - 4] })
    };

    for (i, &(_, key_hint, label)) in FOOTER_ACTIONS.iter().enumerate() {
        app.footer_buttons[i] = button_areas[i];
        let btn_lines = if !footer_text_wraps {
            vec![Line::from(vec![
                Span::styled(
                    format!("{key_hint} "),
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::default().fg(Color::White)),
            ])]
        } else {
            vec![
                Line::from(Span::styled(
                    key_hint,
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(label, Style::default().fg(Color::White))),
            ]
        };
        let btn = Paragraph::new(btn_lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(btn, button_areas[i]);
    }

    // Help & Credits Modal
    if app.show_help {
        let area = centered_rect(74, 80, f.area());
        f.render_widget(Clear, area);
        let easy_n = dict::word_count(mode, crate::game::Difficulty::Easy.scowl_size());
        let med_n = dict::word_count(mode, crate::game::Difficulty::Medium.scowl_size());
        let hard_n = dict::word_count(mode, crate::game::Difficulty::Hard.scowl_size());
        let scrab_n = dict::word_count(mode, crate::game::Difficulty::Scrabble.scowl_size());
        let insane_n = dict::word_count(mode, crate::game::Difficulty::Insane.scowl_size());

        let mut help_text = match mode {
            GameMode::Jotto => vec![
                Line::from(Span::styled(
                    "How to Play Jotto",
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("Jotto is a game where you try to guess the five-letter secret word:"),
                Line::from("  • The secret word has 5 unique letters (no repeating letters)."),
                Line::from("  • The word is not a proper noun."),
                Line::from("  • After each guess, you are told how many letters your guess shares"),
                Line::from("    in common with the secret word (regardless of position)."),
                Line::from(
                    "  • Scratch: Click letters, or [Tab] then type/Space to toggle states.",
                ),
                Line::from(
                    "  • Daily Mode ([^T] or --daily): Deterministic UTC daily word challenge.",
                ),
            ],
            GameMode::Wordle => vec![
                Line::from(Span::styled(
                    "How to Play Wordle",
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("Wordle is a game where you have 6 guesses to find the 5-letter word:"),
                Line::from(
                    "  • Repeating letters are allowed; the word is not a proper noun or plural.",
                ),
                Line::from(vec![
                    Span::raw("  • "),
                    Span::styled(
                        if app.use_color { " X " } else { "+X+" },
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_POSITIVE)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" (Green): Letter is in the word and in the exact spot."),
                ]),
                Line::from(vec![
                    Span::raw("  • "),
                    Span::styled(
                        if app.use_color { " X " } else { "?X?" },
                        Style::default()
                            .fg(Color::Black)
                            .bg(COLOR_WARNING)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" (Orange): Letter is in the word, but in a different spot."),
                ]),
                Line::from(vec![
                    Span::raw("  • "),
                    Span::styled(
                        if app.use_color { " X " } else { "~X~" },
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    Span::raw(" (Gray/Crossed): Letter is not in the word in any spot."),
                ]),
                Line::from(
                    "  • Daily Mode ([^T] or --daily): Deterministic UTC daily word challenge.",
                ),
            ],
        };

        help_text.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Difficulty Levels (SCOWL)",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(format!(
                "  • Easy (35): Recommended small dictionary ({easy_n} words)."
            )),
            Line::from(format!(
                "  • Medium (50): Recommended medium dictionary ({med_n} words)."
            )),
            Line::from(format!(
                "  • Hard (70): Recommended large dictionary ({hard_n} words)."
            )),
            Line::from(format!(
                "  • Scrabble (80): Strange & unusual word-game words ({scrab_n} words)."
            )),
            Line::from(format!(
                "  • 🤷 Insane (95): Every valid 5-letter English word ({insane_n} words)."
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Credits",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  • Created by Gus Lipkin (https://guslipkin.me)"),
            Line::from("  • Word list powered by SCOWL (http://wordlist.aspell.net/)"),
        ]);
        if mode == GameMode::Wordle {
            help_text.push(Line::from(
                "  • Wordle was created by Josh Wardle: https://www.nytimes.com/games/wordle",
            ));
        }
        help_text.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Click anywhere or press [Esc] / [Enter] to close",
                Style::default().fg(Color::DarkGray),
            )),
        ]);

        let modal = Paragraph::new(help_text).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help & Credits ")
                .border_style(Style::default().fg(COLOR_ACCENT)),
        );
        f.render_widget(modal, area);
    }
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
