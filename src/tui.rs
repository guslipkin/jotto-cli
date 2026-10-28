use std::io::{self, stdout};
use std::panic;

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
use crate::game::{GameState, GameStatus, ScratchState};

/// Okabe-Ito colorblind-safe palette
const COLOR_ACCENT: Color = Color::Rgb(86, 180, 233); // Sky Blue (#56B4E9)
const COLOR_POSITIVE: Color = Color::Rgb(0, 158, 115); // Bluish-Green (#009E73)
const COLOR_NEGATIVE: Color = Color::Rgb(213, 94, 0); // Vermilion / Orange-Red (#D55E00)
const COLOR_WARNING: Color = Color::Rgb(230, 159, 0); // Orange (#E69F00)

/// Minimum recommended terminal width in columns.
const MIN_RECOMMENDED_WIDTH: u16 = 70;

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
    input: String,
    focus: FocusPanel,
    scratch_cursor: usize,
    guess_area: Rect,
    scratch_area: Rect,
    footer_buttons: [Rect; 7],
    message: Option<(String, Color)>,
    show_help: bool,
    should_quit: bool,
    table_state: TableState,
}

impl TuiApp {
    fn new(game: GameState) -> Self {
        let initial_message = if let Some(date) = game.daily_date() {
            (
                format!(
                    "Daily Challenge ({date} UTC) started!\nDifficulty: {} ({} words).",
                    game.difficulty(),
                    dict::word_count(game.difficulty().scowl_size())
                ),
                COLOR_ACCENT,
            )
        } else {
            (
                "Enter a 5-letter word with no repeating letters.".to_string(),
                Color::Gray,
            )
        };

        Self {
            game,
            input: String::new(),
            focus: FocusPanel::Guess,
            scratch_cursor: 0,
            guess_area: Rect::default(),
            scratch_area: Rect::default(),
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

        let prev_len = self.game.history().len();
        match self.game.make_guess(&trimmed) {
            Ok(round) => {
                let is_duplicate = self.game.history().len() == prev_len;
                if is_duplicate {
                    let row_idx = prev_len.saturating_sub(round.round);
                    self.table_state.select(Some(row_idx));
                    self.message = Some((
                        format!(
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
                        COLOR_WARNING,
                    ));
                    return;
                }

                self.table_state.select(Some(0));
                if self.game.status() == GameStatus::Won {
                    self.message = Some((
                        format!(
                            "CONGRATULATIONS!!! You guessed {} in {} {}.\nClick [New Game] or press [^N] to play again.",
                            self.game.secret_word().to_ascii_uppercase(),
                            self.game.history().len(),
                            if self.game.history().len() == 1 {
                                "guess"
                            } else {
                                "guesses"
                            }
                        ),
                        COLOR_POSITIVE,
                    ));
                } else {
                    self.message = Some((
                        format!(
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
                        COLOR_ACCENT,
                    ));
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
                dict::word_count(self.game.difficulty().scowl_size())
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
                dict::word_count(self.game.difficulty().scowl_size())
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

        // Check Scratch box click (does not activate/steal focus from Guess box)
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

fn letter_cell_width(inner_width: u16) -> u16 {
    if inner_width >= 13 * 4 {
        4
    } else {
        3
    }
}

fn rect_contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// RAII guard that ensures the terminal is restored on exit or panic unwinding.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
    }
}

/// Runs the full-screen interactive TUI.
pub fn run(game: GameState) -> io::Result<()> {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = TuiApp::new(game);
    let res = run_event_loop(&mut terminal, &mut app);

    let _ = terminal.show_cursor();
    res
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut TuiApp,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw_ui(f, app))?;

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
    if f.area().width < MIN_RECOMMENDED_WIDTH {
        app.guess_area = Rect::default();
        app.scratch_area = Rect::default();
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
            Line::from("Please widen your terminal window, or run `jotto --plain`."),
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
                    .title(" JOTTO ")
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
    let pool_count = dict::word_count(app.game.difficulty().scowl_size());
    let mut header_spans = vec![Span::styled(
        " JOTTO ",
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
    header_spans.extend([
        Span::raw("  Difficulty: "),
        Span::styled(
            format!("{} ({} words)", app.game.difficulty(), pool_count),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  Rounds: "),
        Span::styled(
            app.game.history().len().to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let header =
        Paragraph::new(Line::from(header_spans)).block(Block::default().borders(Borders::ALL));
    f.render_widget(header, chunks[0]);

    // Main split body (Left: Input + Used Letters + Scratch + Status, Right: History Table)
    let body_constraints = if f.area().width < 86 {
        [Constraint::Length(41), Constraint::Min(24)]
    } else {
        [Constraint::Percentage(48), Constraint::Percentage(52)]
    };
    let body_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(body_constraints)
        .split(chunks[1]);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(4),
        ])
        .split(body_chunks[0]);

    app.guess_area = left_chunks[0];
    app.scratch_area = left_chunks[2];

    // Guess Input Box
    let input_title = match app.game.status() {
        GameStatus::Playing => " Guess (5 letters) ",
        GameStatus::Won => " Game Won! Press [^N] for New Game ",
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
    f.render_widget(input_widget, left_chunks[0]);

    let used_cell_w = letter_cell_width(left_chunks[1].width.saturating_sub(2));
    let used_pad = if used_cell_w == 4 { " " } else { "" };

    // Used Letters Box
    let used = app.game.used_letters();
    let mut used_row1 = Vec::new();
    let mut used_row2 = Vec::new();
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
    let used_lines = vec![
        Line::from(""),
        Line::from(used_row1),
        Line::from(""),
        Line::from(used_row2),
    ];
    let used_widget = Paragraph::new(used_lines)
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Used Letters "),
        );
    f.render_widget(used_widget, left_chunks[1]);

    // Scratch Box (3 colorblind-safe states: Default -> ScratchedOut -> Positive)
    let scratch_cell_w = letter_cell_width(left_chunks[2].width.saturating_sub(2));
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
    let scratch_title = if left_chunks[2].width >= 52 {
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
    f.render_widget(scratch_widget, left_chunks[2]);

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
    let status_widget = Paragraph::new(status_lines)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Status ")
                .border_style(Style::default().fg(Color::White)),
        );
    f.render_widget(status_widget, left_chunks[3]);

    // History Table (newest rounds first, matching original Shiny app)
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
        let help_text = vec![
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
            Line::from("  • Scratch: Click letters, or [Tab] then type/Space to toggle states."),
            Line::from("  • Daily Mode ([^T] or --daily): Deterministic UTC daily word challenge."),
            Line::from(""),
            Line::from(Span::styled(
                "Difficulty Levels (SCOWL)",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  • Easy (35): Recommended small dictionary (~2,400 words)."),
            Line::from("  • Medium (50): Recommended medium dictionary (~3,100 words)."),
            Line::from("  • Hard (70): Recommended large dictionary (~4,700 words)."),
            Line::from("  • Scrabble (80): Strange & unusual word-game words (~5,700 words)."),
            Line::from("  • 🤷 Insane (95): Every valid 5-letter English word (~7,400 words)."),
            Line::from(""),
            Line::from(Span::styled(
                "Credits",
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("  • Created by Gus Lipkin (https://guslipkin.me)"),
            Line::from("  • Word list powered by SCOWL (http://wordlist.aspell.net/)"),
            Line::from(""),
            Line::from(Span::styled(
                "Click anywhere or press [Esc] / [Enter] to close",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let modal = Paragraph::new(help_text).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help & Credits ")
                .border_style(Style::default().fg(COLOR_ACCENT)),
        );
        f.render_widget(modal, area);
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
