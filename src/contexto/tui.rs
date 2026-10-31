use std::io;

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};
use ratatui::{Frame, Terminal};

use crate::contexto::data::lexicon;
use crate::contexto::game::{bar_filled, grouped, Closeness, ContextoGame, Difficulty, Status};
use crate::game::supports_color;
use crate::help::{draw_help_page, HelpLine, HelpPage, HelpSection};
use crate::save::Autosave;
use crate::tui::{
    enter_tui, notice_color, rect_contains, COLOR_ACCENT, COLOR_NEGATIVE, COLOR_POSITIVE,
    COLOR_WARNING, MIN_RECOMMENDED_WIDTH,
};

/// Background of the highlighted row in the guess list. A gray background rather
/// than reverse video, which would turn the coloured bars black.
const HIGHLIGHT_BG: Color = Color::DarkGray;

/// Nearest words listed when a game ends.
const NEAREST_SHOWN: usize = 10;
/// Longest word the Guess box accepts (the longest guessable word is shorter).
const MAX_INPUT_LEN: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterAction {
    Guess,
    Help,
    Hint,
    NewGame,
    Daily,
    GiveUp,
    Difficulty,
    Quit,
}

const FOOTER_COUNT: usize = 8;

const FOOTER_ACTIONS: [(FooterAction, &str, &str); FOOTER_COUNT] = [
    (FooterAction::Guess, "[Enter]", "Guess"),
    (FooterAction::Help, "[?]", "Help"),
    (FooterAction::Hint, "[Tab]", "Hint"),
    (FooterAction::NewGame, "[^N]", "New Game"),
    (FooterAction::Daily, "[^T]", "Daily"),
    (FooterAction::GiveUp, "[^G]", "Give Up"),
    (FooterAction::Difficulty, "[^D]", "Difficulty"),
    (FooterAction::Quit, "[Esc]", "Quit"),
];

struct ContextoApp {
    game: ContextoGame,
    use_color: bool,
    input: String,
    footer_buttons: [Rect; FOOTER_COUNT],
    message: (String, Color),
    /// Number of the most recent guess or hint, highlighted in the list.
    latest: Option<usize>,
    show_help: bool,
    should_quit: bool,
    table_state: TableState,
}

impl ContextoApp {
    fn new(game: ContextoGame) -> Self {
        let message = match game.daily_date() {
            Some(date) => (
                format!("Daily Challenge ({date} UTC) started!"),
                COLOR_ACCENT,
            ),
            None => (
                "Guess any word. Ranks show how close its meaning is (#1 is the answer)."
                    .to_string(),
                Color::Gray,
            ),
        };
        Self {
            game,
            use_color: supports_color(),
            input: String::new(),
            footer_buttons: [Rect::default(); FOOTER_COUNT],
            message,
            latest: None,
            show_help: false,
            should_quit: false,
            table_state: TableState::default(),
        }
    }

    /// Selects guess `number` in the closest-first list, so it's highlighted and scrolled into view.
    fn select_guess(&mut self, number: usize) {
        self.latest = Some(number);
        let pos = self
            .game
            .guesses_by_rank()
            .iter()
            .position(|g| g.number == number);
        self.table_state.select(pos);
    }

    fn submit_input(&mut self) {
        let trimmed = self.input.trim().to_ascii_lowercase();
        self.input.clear();
        if trimmed.is_empty() || self.game.status() != Status::Playing {
            return;
        }

        match self.game.make_guess(&trimmed) {
            Ok(outcome) if outcome.duplicate => {
                let g = outcome.guess;
                self.select_guess(g.number);
                self.message = (g.repeat_message(), COLOR_WARNING);
            }
            Ok(outcome) => {
                let g = outcome.guess;
                self.select_guess(g.number);
                if self.game.status() == Status::Won {
                    self.message = (
                        format!(
                            "You found {} in {}!\nClosest words: {}\n[^N] New Game",
                            self.game.secret_word().to_ascii_uppercase(),
                            self.game.count_summary(),
                            self.game.nearest(NEAREST_SHOWN).join(", ")
                        ),
                        COLOR_POSITIVE,
                    );
                } else {
                    let shown = match &g.typed {
                        Some(typed) => format!(
                            "'{}' (as {})",
                            typed.to_ascii_uppercase(),
                            g.word.to_ascii_uppercase()
                        ),
                        None => format!("'{}'", g.word.to_ascii_uppercase()),
                    };
                    self.message = (format!("{shown} is #{}.", g.rank), closeness_color(g.rank));
                }
            }
            Err(err) => self.message = (err.to_string(), COLOR_WARNING),
        }
    }

    fn take_hint(&mut self) {
        if self.game.status() != Status::Playing {
            return;
        }
        match self.game.hint() {
            Ok(g) => {
                self.select_guess(g.number);
                self.message = (g.hint_message(), COLOR_ACCENT);
            }
            Err(err) => self.message = (err.to_string(), COLOR_WARNING),
        }
    }

    fn start_new_game(&mut self, difficulty: Option<Difficulty>) {
        self.game.reset(difficulty);
        let d = self.game.difficulty();
        self.after_restart(format!(
            "New game started!\nDifficulty: {d} ({}; {} possible answers).",
            d.description(),
            grouped(d.secret_count())
        ));
    }

    fn cycle_difficulty(&mut self) {
        let next = self.game.difficulty().next();
        self.start_new_game(Some(next));
    }

    fn start_daily_game(&mut self) {
        self.game.start_daily();
        let date = self.game.daily_date().unwrap_or("today").to_string();
        self.after_restart(format!(
            "Daily Challenge ({date} UTC) started!\nDifficulty: {}.",
            self.game.difficulty()
        ));
    }

    fn after_restart(&mut self, message: String) {
        self.input.clear();
        self.latest = None;
        self.table_state.select(None);
        self.message = (message, COLOR_ACCENT);
    }

    fn trigger_give_up(&mut self) {
        if self.game.status() != Status::Playing {
            return;
        }
        self.game.give_up();
        self.input.clear();
        self.message = (
            format!(
                "The word was: {}\nClosest words: {}\n[^N] New Game",
                self.game.secret_word().to_ascii_uppercase(),
                self.game.nearest(NEAREST_SHOWN).join(", ")
            ),
            COLOR_WARNING,
        );
    }

    fn scroll_up(&mut self) {
        if !self.game.guesses().is_empty() {
            let i = self.table_state.selected().unwrap_or(0);
            self.table_state.select(Some(i.saturating_sub(1)));
        }
    }

    fn scroll_down(&mut self) {
        let len = self.game.guesses().len();
        if len > 0 {
            let i = self.table_state.selected().unwrap_or(0);
            self.table_state.select(Some((i + 1).min(len - 1)));
        }
    }

    fn execute_footer_action(&mut self, action: FooterAction) {
        match action {
            FooterAction::Guess => self.submit_input(),
            FooterAction::Help => self.show_help = !self.show_help,
            FooterAction::Hint => self.take_hint(),
            FooterAction::NewGame => self.start_new_game(None),
            FooterAction::Daily => self.start_daily_game(),
            FooterAction::GiveUp => self.trigger_give_up(),
            FooterAction::Difficulty => self.cycle_difficulty(),
            FooterAction::Quit => self.should_quit = true,
        }
    }

    fn handle_mouse_click(&mut self, col: u16, row: u16) {
        if self.show_help {
            self.show_help = false;
            return;
        }
        if let Some(i) = self
            .footer_buttons
            .iter()
            .position(|&r| rect_contains(r, col, row))
        {
            self.execute_footer_action(FOOTER_ACTIONS[i].0);
        }
    }
}

/// Colour for a rank's [`Closeness`], shared with plain mode.
pub(super) fn closeness_color(rank: usize) -> Color {
    match Closeness::of(rank) {
        Closeness::Close => COLOR_POSITIVE,
        Closeness::Warm => COLOR_WARNING,
        Closeness::Far => COLOR_NEGATIVE,
    }
}

/// A closeness bar `width` cells wide. With colour, the unfilled part is blank, so it
/// takes the background behind it (gray on the highlighted row). Without colour, the
/// bar is `#` on `.` so its length still reads.
fn bar_spans(rank: usize, width: u16, use_color: bool) -> Vec<Span<'static>> {
    let width = width as usize;
    let filled = bar_filled(rank, width);
    let (fill, empty) = if use_color { ("█", " ") } else { ("#", ".") };
    vec![
        Span::styled(
            fill.repeat(filled),
            Style::default().fg(closeness_color(rank)),
        ),
        Span::raw(empty.repeat(width - filled)),
    ]
}

/// Runs the full-screen interactive TUI.
pub fn run(game: ContextoGame, autosave: &mut Autosave) -> io::Result<()> {
    let mut app = ContextoApp::new(game);
    if let Some(notice) = autosave.notice.take() {
        if let Some(last) = app.game.guesses().last() {
            app.select_guess(last.number);
        }
        app.message = (notice.text().to_string(), notice_color(&notice));
    }
    let (_guard, mut terminal) = enter_tui()?;
    run_event_loop(&mut terminal, &mut app, autosave)
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut ContextoApp,
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
                    app.handle_mouse_click(mouse.column, mouse.row)
                }
                MouseEventKind::ScrollUp if !app.show_help => app.scroll_up(),
                MouseEventKind::ScrollDown if !app.show_help => app.scroll_down(),
                _ => {}
            },
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('c') {
                    return Ok(());
                }
                if app.show_help {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q')
                    ) {
                        app.show_help = false;
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Char('?') => app.show_help = true,
                    KeyCode::Tab => app.take_hint(),
                    KeyCode::Char('n') if ctrl => app.start_new_game(None),
                    KeyCode::Char('d') if ctrl => app.cycle_difficulty(),
                    KeyCode::Char('t') if ctrl => app.start_daily_game(),
                    KeyCode::Char('g') if ctrl => app.trigger_give_up(),
                    KeyCode::Enter => app.submit_input(),
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Up => app.scroll_up(),
                    KeyCode::Down => app.scroll_down(),
                    KeyCode::Char(c)
                        if c.is_ascii_alphabetic()
                            && !ctrl
                            && app.game.status() == Status::Playing
                            && app.input.len() < MAX_INPUT_LEN =>
                    {
                        app.input.push(c.to_ascii_lowercase());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn draw_ui(f: &mut Frame, app: &mut ContextoApp) {
    let title = " CONTEXTO ";

    if f.area().width < MIN_RECOMMENDED_WIDTH {
        app.footer_buttons = [Rect::default(); FOOTER_COUNT];
        let warning = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Terminal Width Too Narrow",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "Current width: {} columns (minimum recommended: {MIN_RECOMMENDED_WIDTH} columns).",
                f.area().width
            )),
            Line::from("Please widen your terminal window, or run `contexto --plain`."),
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
                .title(title)
                .border_style(Style::default().fg(COLOR_WARNING)),
        );
        f.render_widget(warning, f.area());
        return;
    }

    // Footer on one row when every button's "[key] label" fits, else on two (4 + 4).
    let one_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, FOOTER_COUNT as u32); FOOTER_COUNT])
        .split(f.area());
    let fits_one_row = FOOTER_ACTIONS.iter().enumerate().all(|(i, &(_, k, l))| {
        (k.len() + 1 + l.len()) as u16 <= one_row[i].width.saturating_sub(2)
    });
    let footer_height = if fits_one_row { 3 } else { 6 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(footer_height),
        ])
        .split(f.area());

    // Header
    let mut header_spans = vec![Span::styled(
        title,
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
    let bold_white = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    header_spans.extend([
        Span::raw("  Difficulty: "),
        Span::styled(
            app.game.difficulty().label(),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  |  Guesses: "),
        Span::styled(app.game.guess_count().to_string(), bold_white),
        Span::raw("  |  Hints: "),
        Span::styled(app.game.hint_count().to_string(), bold_white),
        Span::raw("  |  Best: "),
        match app.game.best_rank() {
            Some(r) => Span::styled(
                format!("#{r}"),
                Style::default()
                    .fg(closeness_color(r))
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::styled("-", bold_white),
        },
    ]);
    f.render_widget(
        Paragraph::new(Line::from(header_spans)).block(Block::default().borders(Borders::ALL)),
        chunks[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[1]);
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(4),
        ])
        .split(body[0]);

    // Guess input
    let input_title = match app.game.status() {
        Status::Playing => " Guess (any word) ",
        Status::Won => " You Won! Press [^N] for New Game ",
        Status::GivenUp => " Game Over! Press [^N] for New Game ",
    };
    let cursor = if app.game.status() == Status::Playing {
        "_"
    } else {
        ""
    };
    f.render_widget(
        Paragraph::new(format!("{}{cursor}", app.input.to_ascii_uppercase()))
            .style(bold_white)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(input_title)
                    .border_style(Style::default().fg(COLOR_ACCENT)),
            ),
        left[0],
    );

    // Latest guess, so it's visible even when it ranks below the list's visible rows.
    let latest_block = Block::default().borders(Borders::ALL).title(" Latest ");
    let latest_inner = latest_block.inner(left[1]);
    f.render_widget(latest_block, left[1]);
    if let Some(g) = app
        .latest
        .and_then(|n| app.game.guesses().iter().find(|g| g.number == n))
    {
        let rank_text = format!("#{}", g.rank);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{rank_text:<8}"),
                        Style::default()
                            .fg(closeness_color(g.rank))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(g.label(), bold_white),
                ]),
                Line::from(bar_spans(g.rank, latest_inner.width, app.use_color)),
            ]),
            latest_inner,
        );
    } else {
        f.render_widget(
            Paragraph::new(Span::styled(
                "No guesses yet.",
                Style::default().fg(Color::DarkGray),
            )),
            latest_inner,
        );
    }

    // Status
    let (msg, msg_color) = &app.message;
    f.render_widget(
        Paragraph::new(
            msg.lines()
                .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(*msg_color))))
                .collect::<Vec<_>>(),
        )
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Status ")),
        left[2],
    );

    // Closest-first list of every guess and hint
    let list_inner_w = body[1].width.saturating_sub(2);
    let rank_w = 7u16;
    let word_w = (list_inner_w / 2).clamp(12, 26);
    let bar_w = list_inner_w.saturating_sub(rank_w + word_w + 2).max(4);
    let latest = app.latest;
    let rows: Vec<Row> = app
        .game
        .guesses_by_rank()
        .into_iter()
        .map(|g| {
            let mut word_style = Style::default().fg(Color::White);
            if Some(g.number) == latest {
                word_style = word_style.add_modifier(Modifier::BOLD);
            }
            if g.hint {
                word_style = word_style.fg(COLOR_ACCENT);
            }
            Row::new(vec![
                Cell::from(format!("#{}", g.rank))
                    .style(Style::default().fg(closeness_color(g.rank))),
                Cell::from(g.label()).style(word_style),
                Cell::from(Line::from(bar_spans(g.rank, bar_w, app.use_color))),
            ])
        })
        .collect();
    let header = Row::new(["Rank", "Word", "Closeness"].map(|h| {
        Cell::from(h).style(
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    }))
    .bottom_margin(1);
    let table = Table::new(
        rows,
        [
            Constraint::Length(rank_w),
            Constraint::Length(word_w),
            Constraint::Length(bar_w),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Closest Guesses (* = hint) "),
    )
    .row_highlight_style(if app.use_color {
        Style::default().bg(HIGHLIGHT_BG)
    } else {
        // Without colour a background doesn't show, so fall back to reverse video.
        Style::default().add_modifier(Modifier::REVERSED)
    });
    f.render_stateful_widget(table, body[1], &mut app.table_state);

    // Clickable footer buttons
    let button_areas: [Rect; FOOTER_COUNT] = if fits_one_row {
        let row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, FOOTER_COUNT as u32); FOOTER_COUNT])
            .split(chunks[2]);
        std::array::from_fn(|i| row[i])
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Length(3)])
            .split(chunks[2]);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 4); 4])
            .split(rows[0]);
        let bot = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, 4); 4])
            .split(rows[1]);
        std::array::from_fn(|i| if i < 4 { top[i] } else { bot[i - 4] })
    };
    for (i, &(_, key_hint, label)) in FOOTER_ACTIONS.iter().enumerate() {
        app.footer_buttons[i] = button_areas[i];
        let btn = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{key_hint} "),
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(Color::White)),
        ]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(btn, button_areas[i]);
    }

    if app.show_help {
        draw_help(f, app.use_color);
    }
}

/// The help page: rules with the closeness colours, controls, levels, credits.
fn draw_help(f: &mut Frame, use_color: bool) {
    let lex = lexicon();
    let mut closeness = vec![];
    for (rank, label) in [
        (50, " Close ≤#300  "),
        (800, " Warm ≤#1,500  "),
        (8000, " Far"),
    ] {
        closeness.extend(bar_spans(rank, 6, use_color));
        closeness.push(Span::raw(label));
    }
    let page = HelpPage {
        game: "Contexto",
        tagline: "find the secret noun by its meaning".to_string(),
        sections: vec![
            HelpSection {
                title: "Rules",
                lines: vec![
                    HelpLine::Bullet(Line::from(format!(
                        "{} words accepted, ranked out of {}; #1 is the answer.",
                        grouped(lex.guessable_count()),
                        grouped(lex.ranked_count())
                    ))),
                    HelpLine::Bullet(Line::from(
                        "Plurals and other forms count as their base ('mice' is 'mouse').",
                    )),
                    HelpLine::Bullet(Line::from(
                        "Closeness comes from usage, so a shared topic counts too.",
                    )),
                    HelpLine::Bullet(Line::from(closeness)),
                    HelpLine::Bullet(Line::from(
                        "A hint shows the word at half your best guess's rank.",
                    )),
                ],
            },
            HelpSection {
                title: "Controls",
                lines: vec![
                    HelpLine::Keys(vec![
                        ("Enter", "guess"),
                        ("Tab", "hint"),
                        ("↑↓", "scroll"),
                        ("Esc", "quit"),
                    ]),
                    HelpLine::Keys(vec![
                        ("^N", "new"),
                        ("^T", "daily (medium)"),
                        ("^G", "give up"),
                        ("^D", "difficulty"),
                    ]),
                ],
            },
            HelpSection {
                title: "Difficulty · possible answers",
                lines: [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard]
                    .iter()
                    .map(|&d| HelpLine::Row {
                        name: d.label().to_string(),
                        desc: d.description().to_string(),
                        note: grouped(d.secret_count()),
                    })
                    .collect(),
            },
            HelpSection {
                title: "Credits",
                lines: vec![
                    HelpLine::Credit("By", "Gus Lipkin (https://guslipkin.me)".to_string()),
                    HelpLine::Credit("Inspired by", "Contexto: https://contexto.me/".to_string()),
                    HelpLine::Credit(
                        "Data",
                        "Words: SCOWL. Meanings: GloVe. Difficulty: WordNet.".to_string(),
                    ),
                ],
            },
        ],
    };
    draw_help_page(f, &page);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn render(app: &mut ContextoApp, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw_ui(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app_with_guesses(word: &str, guesses: &[&str]) -> ContextoApp {
        let mut app = ContextoApp::new(ContextoGame::new(Difficulty::Medium, Some(word)).unwrap());
        for g in guesses {
            app.input = g.to_string();
            app.submit_input();
        }
        app
    }

    #[test]
    fn renders_guesses_closest_first() {
        let mut app = app_with_guesses("dog", &["animal", "mice", "horse"]);
        app.take_hint();
        let screen = render(&mut app, 100, 30);
        if std::env::var_os("CONTEXTO_SHOW_TUI").is_some() {
            println!("{screen}");
        }
        assert!(screen.contains("CONTEXTO"));
        assert!(screen.contains("Guesses: 3"));
        assert!(screen.contains("Hints: 1"));
        assert!(screen.contains("MOUSE (mice)"));
        let rows: Vec<usize> = ["HORSE", "ANIMAL", "MOUSE"]
            .iter()
            .map(|w| screen.lines().position(|l| l.contains(w)).unwrap())
            .collect();
        assert!(
            rows.windows(2).all(|p| p[0] < p[1]),
            "list isn't closest-first"
        );
    }

    #[test]
    fn help_fits_a_small_terminal() {
        let mut app = app_with_guesses("dog", &[]);
        app.show_help = true;
        let screen = render(&mut app, 80, 24);
        if std::env::var_os("CONTEXTO_SHOW_TUI").is_some() {
            println!("{screen}");
        }
        for text in ["How to Play", "Hard", "adds ideas", "WordNet", "to close"] {
            assert!(screen.contains(text), "help is cut off before '{text}'");
        }
    }

    #[test]
    fn highlighted_row_is_gray_through_the_bar() {
        let mut app = app_with_guesses("dog", &["cat", "violin"]);
        app.use_color = true;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|f| draw_ui(f, &mut app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let row_of = |word: &str| {
            (0..30u16)
                .find(|&y| {
                    (50..100u16)
                        .map(|x| buf[(x, y)].symbol())
                        .collect::<String>()
                        .contains(word)
                })
                .unwrap()
        };
        // The latest guess (VIOLIN) is the highlighted row; CAT isn't.
        for (y, highlighted) in [(row_of("VIOLIN"), true), (row_of("CAT"), false)] {
            // Columns 81..99 are the bar, between the word column and the border.
            let bar: Vec<_> = (81..99u16).map(|x| &buf[(x, y)]).collect();
            let filled: Vec<_> = bar.iter().filter(|c| c.symbol() == "█").collect();
            assert!(!filled.is_empty(), "the bar has no filled part");
            assert!(
                filled.iter().all(|c| c.fg != Color::Reset),
                "the bar lost its colour"
            );
            let empty: Vec<_> = bar.iter().filter(|c| c.symbol() == " ").collect();
            assert!(!empty.is_empty(), "the bar has no unfilled part");
            assert_eq!(filled.len() + empty.len(), bar.len());
            let want_bg = if highlighted {
                HIGHLIGHT_BG
            } else {
                Color::Reset
            };
            for c in &empty {
                assert_eq!(c.bg, want_bg, "the unfilled part has the wrong background");
            }
        }
    }

    #[test]
    fn narrow_terminal_shows_warning() {
        let mut app = app_with_guesses("dog", &[]);
        let screen = render(&mut app, 60, 20);
        assert!(screen.contains("Terminal Width Too Narrow"));
    }

    #[test]
    fn win_and_footer_click() {
        let mut app = app_with_guesses("dog", &["dog"]);
        let screen = render(&mut app, 80, 24);
        assert!(screen.contains("You found DOG in 1 guess"));
        // Clicking New Game (footer button 4) starts a fresh game.
        let btn = app.footer_buttons[3];
        app.handle_mouse_click(btn.x + 1, btn.y + 1);
        assert_eq!(app.game.status(), Status::Playing);
        assert!(app.game.guesses().is_empty());
    }
}
