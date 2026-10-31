//! `arcade`: all four games in one program, chosen from a menu.
//!
//! Choosing a game plays it, in its full-screen TUI, inside this program; when
//! the game ends, however it ends, the menu comes back. Quitting the menu exits
//! to wherever it was started from. Games resume and save as their own commands
//! do (in `~/.ndt`), so a game left in `arcade` carries on in `flights`, say.
//!
//! The Docker image (see `Dockerfile`) runs it as each browser session's only
//! program, with `--session-saves`: each session then keeps its saves in a folder
//! of its own, removed when it ends, so players connected at once never share or
//! keep each other's games. Since the games run inside it, it never starts
//! another program, so nothing a player does reaches a command line.

use std::io::{self, stdout, Stdout};
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use jotto::game::GameMode;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::{Frame, Terminal};

/// Pick any of the four games and play it; leaving a game comes back here.
#[derive(Debug, Parser)]
#[command(name = "arcade", version, about, long_about = None)]
struct ArcadeArgs {
    /// Keep this session's saves in a folder of its own, removed when it ends,
    /// instead of ~/.ndt (what the Docker image does, so players don't share them)
    #[arg(long)]
    session_saves: bool,
}

/// Sky blue, the games' accent colour (Okabe-Ito).
const ACCENT: Color = Color::Rgb(86, 180, 233);
/// Orange, for a game that couldn't start (Okabe-Ito).
const WARNING: Color = Color::Rgb(230, 159, 0);

/// A game's entry: it plays the game in its TUI and returns when it ends.
type Play = fn() -> Result<(), String>;

/// The games in alphabetical order, as `(play it, name, what it is)`.
const GAMES: [(Play, &str, &str); 4] = [
    (
        jotto::contexto::play,
        "Contexto",
        "find the secret noun by its meaning",
    ),
    (
        jotto::flights::play,
        "Flights",
        "the shortest way between airports, a letter at a time",
    ),
    (
        || jotto::play(GameMode::Jotto),
        "Jotto",
        "guess the five-letter word, unlimited guesses",
    ),
    (
        || jotto::play(GameMode::Wordle),
        "Wordle",
        "find the five-letter word in six guesses",
    ),
];

/// The keys, along the menu's bottom border.
const FOOTER: &str = " [↑↓] choose  [Enter] play  [1-4] play  [q] quit ";

/// The menu's rows: the games, then Quit.
const ROWS: usize = GAMES.len() + 1;

/// A save folder for this session alone, removed when the session ends (even on
/// a panic, as the guard drops while unwinding).
struct SessionDir(PathBuf);

impl SessionDir {
    fn create() -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = env::temp_dir().join(format!("arcade-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for SessionDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The full-screen terminal for the menu, restored when it drops, so a game
/// starts on the normal screen and sets the terminal up as it likes.
struct Screen(Terminal<CrosstermBackend<Stdout>>);

impl Screen {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(Self(Terminal::new(CrosstermBackend::new(stdout()))?))
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

fn main() -> io::Result<()> {
    let args = ArcadeArgs::parse();
    // Kept alive until the menu is quit, then removed with its saves.
    let _session = if args.session_saves {
        let dir = SessionDir::create()?;
        // Read by each game's save code; set before anything else runs.
        env::set_var("NDT_SAVE_DIR", &dir.0);
        Some(dir)
    } else {
        None
    };
    let mut selected = 0;
    let mut note: Option<String> = None;
    loop {
        let choice = {
            let mut screen = Screen::enter()?;
            pick(&mut screen.0, &mut selected, note.take())?
        };
        let Some(i) = choice else {
            return Ok(());
        };
        let (play, name, _) = GAMES[i];
        // However the game ends, even by a panic (the game's own terminal guard
        // puts the screen back as it unwinds), the menu comes back.
        note = match panic::catch_unwind(AssertUnwindSafe(play)) {
            Ok(Ok(())) => None,
            Ok(Err(e)) => Some(format!("{name}: {e}")),
            Err(_) => Some(format!("{name} stopped unexpectedly.")),
        };
    }
}

/// Shows the menu until a game is chosen (`Some` of its index) or Quit (`None`).
fn pick(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    selected: &mut usize,
    note: Option<String>,
) -> io::Result<Option<usize>> {
    loop {
        terminal.draw(|f| draw(f, *selected, note.as_deref()))?;
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('c') | KeyCode::Char('d') if ctrl => return Ok(None),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Up | KeyCode::Char('k') => *selected = (*selected + ROWS - 1) % ROWS,
            KeyCode::Down | KeyCode::Char('j') => *selected = (*selected + 1) % ROWS,
            KeyCode::Char(c @ '1'..='4') => {
                return Ok(Some(c as usize - '1' as usize));
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char(' ') => {
                return Ok((*selected < GAMES.len()).then_some(*selected));
            }
            _ => {}
        }
    }
}

fn draw(f: &mut Frame, selected: usize, note: Option<&str>) {
    // As wide as the longest line (" > " + " 1  Contexto   " + "  " + what it
    // is), plus borders and a margin, so nothing is cut; no wider than the terminal.
    let longest = GAMES
        .iter()
        .map(|&(_, _, about)| 3 + 13 + 2 + about.chars().count())
        .max()
        .unwrap_or(0)
        .max(FOOTER.chars().count());
    let width = (longest as u16 + 4).min(f.area().width);
    let height = (ROWS as u16 * 2 + 8).min(f.area().height);
    let area = Rect {
        x: f.area().x + (f.area().width - width) / 2,
        y: f.area().y + (f.area().height - height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Line::from(" Arcade ").centered())
        .title_bottom(
            Line::from(Span::styled(FOOTER, Style::default().fg(Color::Gray))).centered(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "  Choose a game",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                "  Leaving a game brings you back here.",
                Style::default().fg(Color::Gray),
            )),
        ]),
        rows[0],
    );
    let entries = GAMES
        .iter()
        .enumerate()
        .map(|(i, &(_, name, about))| (i, format!("{}  {name}", i + 1), about))
        .chain([(GAMES.len(), "   Quit".to_string(), "end this session")]);
    let mut lines = vec![Line::from("")];
    for (i, label, about) in entries {
        let chosen = i == selected;
        let marker = if chosen { " > " } else { "   " };
        let style = if chosen {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        lines.push(Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(format!(" {label:<12}"), style),
            Span::styled(format!("  {about}"), Style::default().fg(Color::Gray)),
        ]));
        lines.push(Line::from(""));
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    if let Some(note) = note {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {note}"),
                Style::default().fg(WARNING),
            )),
            rows[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn menu_lists_every_game_and_quit() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| draw(f, 2, Some("Contexto stopped")))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for text in [
            "Arcade", "Jotto", "Wordle", "Contexto", "Flights", "Quit", "stopped",
        ] {
            assert!(screen.contains(text), "no '{text}'");
        }
        // Every description shows whole.
        for (_, _, about) in GAMES {
            assert!(screen.contains(about), "'{about}' is cut off");
        }
        // Alphabetical.
        let at = |name: &str| screen.find(name).unwrap();
        assert!(at("Contexto") < at("Flights"));
        assert!(at("Flights") < at("Jotto") && at("Jotto") < at("Wordle"));
    }

    #[test]
    fn session_folder_is_removed() {
        let dir = SessionDir::create().unwrap();
        let path = dir.0.clone();
        fs::write(path.join("flights.json"), "{}").unwrap();
        drop(dir);
        assert!(!path.exists());
    }
}
