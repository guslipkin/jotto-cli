//! `arcade`: all four games in one program, chosen from a menu.
//!
//! It's also each game's command: run as `arcade flights ...`, or under a game's
//! name (a link or copy called `flights`, which is how the installers make the
//! four commands), it is that game's command, with all its options. One program
//! then serves all five names, instead of five downloads that each carry the
//! same games' data.
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

use std::ffi::{OsStr, OsString};
use std::io::{self, Stdout};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{env, fs};

use clap::{Args, FromArgMatches, Parser, Subcommand};
use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use jotto::game::GameMode;
use jotto::tui::{black_on, bold, enter_tui, rect_contains, COLOR_ACCENT, COLOR_WARNING};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::{Frame, Terminal};

/// Pick any of the four games and play it; leaving a game comes back here.
#[derive(Debug, Parser)]
#[command(name = "arcade", version, about, long_about = None)]
#[command(args_conflicts_with_subcommands = true)]
struct ArcadeArgs {
    /// Keep this session's saves in a folder of its own, removed when it ends,
    /// instead of ~/.ndt (what the Docker image does, so players don't share them)
    #[arg(long)]
    session_saves: bool,

    /// Play one game directly, as its own command would, instead of the menu
    #[command(subcommand)]
    game: Option<GameCommand>,
}

/// `arcade <game> ...`: the game's name, then its command's own options. The
/// subcommands are added from [`GAMES`], so this only collects what follows.
#[derive(Debug)]
struct GameCommand {
    command: String,
    args: Vec<OsString>,
}

/// Everything after the game's name, untouched, so the game's own parser sees
/// `--help`, `--plain` and the rest.
#[derive(Debug, Args)]
#[command(disable_help_flag = true)]
struct PassThrough {
    /// The game's options (see `arcade <game> --help`)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<OsString>,
}

impl FromArgMatches for GameCommand {
    fn from_arg_matches(matches: &clap::ArgMatches) -> Result<Self, clap::Error> {
        let (command, sub) = matches
            .subcommand()
            .ok_or_else(|| clap::Error::new(clap::error::ErrorKind::MissingSubcommand))?;
        Ok(Self {
            command: command.to_string(),
            args: PassThrough::from_arg_matches(sub)?.args,
        })
    }

    fn update_from_arg_matches(&mut self, matches: &clap::ArgMatches) -> Result<(), clap::Error> {
        *self = Self::from_arg_matches(matches)?;
        Ok(())
    }
}

impl Subcommand for GameCommand {
    fn augment_subcommands(cmd: clap::Command) -> clap::Command {
        GAMES.iter().fold(cmd, |cmd, game| {
            // The about comes after the args, which would otherwise replace it
            // with `PassThrough`'s doc comment.
            cmd.subcommand(
                PassThrough::augment_args(clap::Command::new(game.command))
                    .about(format!("Play {}: {}", game.name, game.about)),
            )
        })
    }

    fn augment_subcommands_for_update(cmd: clap::Command) -> clap::Command {
        Self::augment_subcommands(cmd)
    }

    fn has_subcommand(name: &str) -> bool {
        GAMES.iter().any(|game| game.command == name)
    }
}

/// A game's entry: it plays the game in its TUI and returns when it ends.
type Play = fn() -> Result<(), String>;

/// A game's command: it runs the command line given (its name first) as that
/// game's own program would.
type Cli = fn(Vec<OsString>);

/// One of the games: how the menu plays it, and how its command runs.
struct Game {
    play: Play,
    cli: Cli,
    /// Its name in the menu.
    name: &'static str,
    /// Its command, and `arcade`'s subcommand for it.
    command: &'static str,
    /// What it is, beside its name.
    about: &'static str,
}

/// The games in alphabetical order.
const GAMES: [Game; 4] = [
    Game {
        play: jotto::contexto::play,
        cli: jotto::contexto::run_cli,
        name: "Contexto",
        command: "contexto",
        about: "find the secret noun by its meaning",
    },
    Game {
        play: jotto::flights::play,
        cli: jotto::flights::run_cli,
        name: "Flights",
        command: "flights",
        about: "the shortest way between airports, a letter at a time",
    },
    Game {
        play: || jotto::play(GameMode::Jotto),
        cli: |args| jotto::run_cli(GameMode::Jotto, args),
        name: "Jotto",
        command: "jotto",
        about: "guess the five-letter word, unlimited guesses",
    },
    Game {
        play: || jotto::play(GameMode::Wordle),
        cli: |args| jotto::run_cli(GameMode::Wordle, args),
        name: "Wordle",
        command: "wordle",
        about: "find the five-letter word in six guesses",
    },
];

/// The game whose command `program` (how this was run: its first argument) is,
/// if it's named after one: `flights`, `/usr/local/bin/flights` or `FLIGHTS.EXE`.
fn game_named(program: &OsStr) -> Option<&'static Game> {
    let stem = Path::new(program).file_stem()?.to_str()?;
    GAMES
        .iter()
        .find(|game| game.command.eq_ignore_ascii_case(stem))
}

/// The keys, along the menu's bottom border.
const FOOTER: &str = " [↑↓] choose  [Enter] or click: play  [1-4] play  [q] quit ";

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

fn main() -> io::Result<()> {
    // Run under a game's name, this is that game's command, options and all.
    if let Some(game) = env::args_os().next().as_deref().and_then(game_named) {
        (game.cli)(env::args_os().collect());
        return Ok(());
    }
    let args = ArcadeArgs::parse();
    if let Some(GameCommand { command, args }) = args.game {
        let game = GAMES.iter().find(|game| game.command == command);
        // Named `arcade flights` in the game's usage and errors, as that's how
        // it was run.
        let program = OsString::from(format!("arcade {command}"));
        (game.expect("clap only accepts the games' commands").cli)(
            std::iter::once(program).chain(args).collect(),
        );
        return Ok(());
    }
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
        // The menu's terminal is put back before a game starts, so the game
        // sets it up as it likes.
        let choice = {
            let (_guard, mut terminal) = enter_tui()?;
            pick(&mut terminal, &mut selected, note.take())?
        };
        let Some(i) = choice else {
            return Ok(());
        };
        let Game { play, name, .. } = GAMES[i];
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
    // Where each row was drawn, for the mouse.
    let mut items = [Rect::default(); ROWS];
    loop {
        terminal.draw(|f| items = draw(f, *selected, note.as_deref()))?;
        let key = match jotto::input::read()? {
            Event::Key(key) => key,
            // Hovering a row chooses it, clicking it plays it (or quits), and the
            // wheel moves the choice.
            Event::Mouse(mouse) => {
                let over = row_at(&items, mouse.column, mouse.row);
                match (mouse.kind, over) {
                    (MouseEventKind::Moved, Some(i)) => *selected = i,
                    (MouseEventKind::Down(MouseButton::Left), Some(i)) => {
                        *selected = i;
                        return Ok((i < GAMES.len()).then_some(i));
                    }
                    (MouseEventKind::ScrollUp, _) => *selected = (*selected + ROWS - 1) % ROWS,
                    (MouseEventKind::ScrollDown, _) => *selected = (*selected + 1) % ROWS,
                    _ => {}
                }
                continue;
            }
            _ => continue,
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

/// The row (a game, or Quit) at terminal cell (`col`, `row`), if any.
fn row_at(items: &[Rect], col: u16, row: u16) -> Option<usize> {
    items.iter().position(|&r| rect_contains(r, col, row))
}

/// Draws the menu, returning where each row went, so the mouse can find them.
fn draw(f: &mut Frame, selected: usize, note: Option<&str>) -> [Rect; ROWS] {
    // As wide as the longest line (" > " + " 1  Contexto   " + "  " + what it
    // is), plus borders and a margin, so nothing is cut; no wider than the terminal.
    let longest = GAMES
        .iter()
        .map(|game| 3 + 13 + 2 + game.about.chars().count())
        .max()
        .unwrap_or(0)
        .max(FOOTER.chars().count());
    let width = (longest as u16 + 4).min(f.area().width);
    // Too narrow for a description beside its game (a phone held upright, say):
    // it goes on the line below instead, which is blank otherwise.
    let stacked = usize::from(width) < longest + 4;
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
        .border_style(Style::default().fg(COLOR_ACCENT))
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
            Line::from(Span::styled("  Choose a game", bold(Color::White))),
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
        .map(|(i, game)| (i, format!("{}  {}", i + 1, game.name), game.about))
        .chain([(GAMES.len(), "   Quit".to_string(), "end this session")]);
    let mut lines = vec![Line::from("")];
    for (i, label, about) in entries {
        let chosen = i == selected;
        let marker = if chosen { " > " } else { "   " };
        let style = if chosen {
            black_on(COLOR_ACCENT)
        } else {
            Style::default().fg(Color::White)
        };
        let about = Span::styled(about, Style::default().fg(Color::Gray));
        if stacked {
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(COLOR_ACCENT)),
                Span::styled(format!(" {label:<12}"), style),
            ]));
            lines.push(Line::from(vec![Span::raw("      "), about]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(COLOR_ACCENT)),
                Span::styled(format!(" {label:<12}"), style),
                Span::raw("  "),
                about,
            ]));
            lines.push(Line::from(""));
        }
    }
    f.render_widget(Paragraph::new(lines), rows[1]);
    // Each row is a line, after a blank one, with a blank between rows; a row
    // that didn't fit the terminal has no area.
    let list = rows[1];
    let items = std::array::from_fn(|i| {
        let y = list.y + 1 + 2 * i as u16;
        if y < list.y + list.height {
            Rect {
                y,
                height: 1,
                ..list
            }
        } else {
            Rect::default()
        }
    });
    if let Some(note) = note {
        f.render_widget(
            Paragraph::new(Span::styled(
                format!("  {note}"),
                Style::default().fg(COLOR_WARNING),
            )),
            rows[2],
        );
    }
    // True black behind everything, unless colours are off.
    if jotto::game::supports_color() {
        jotto::tui::paint_background(f.buffer_mut());
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn menu_lists_every_game_and_quit() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|f| {
                draw(f, 2, Some("Contexto stopped"));
            })
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
        for Game { about, .. } in GAMES {
            assert!(screen.contains(about), "'{about}' is cut off");
        }
        // Alphabetical.
        let at = |name: &str| screen.find(name).unwrap();
        assert!(at("Contexto") < at("Flights"));
        assert!(at("Flights") < at("Jotto") && at("Jotto") < at("Wordle"));
    }

    #[test]
    fn rows_are_found_where_they_are_drawn() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut items = [Rect::default(); ROWS];
        terminal.draw(|f| items = draw(f, 0, None)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let line = |y: u16| (0..80u16).map(|x| buf[(x, y)].symbol()).collect::<String>();
        for (i, &Game { name, .. }) in GAMES.iter().enumerate() {
            assert!(line(items[i].y).contains(name), "row {i} isn't {name}");
            let (x, y) = (items[i].x + 5, items[i].y);
            assert_eq!(row_at(&items, x, y), Some(i));
        }
        assert!(line(items[GAMES.len()].y).contains("Quit"));
        assert_eq!(
            row_at(&items, items[0].x, items[0].y + 1),
            None,
            "the gap between rows"
        );
        assert_eq!(row_at(&items, 0, 0), None);
    }

    #[test]
    fn narrow_screens_put_descriptions_under_names() {
        let mut terminal = Terminal::new(TestBackend::new(44, 30)).unwrap();
        let mut items = [Rect::default(); ROWS];
        terminal.draw(|f| items = draw(f, 0, None)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let line = |y: u16| (0..44u16).map(|x| buf[(x, y)].symbol()).collect::<String>();
        for (i, &Game { name, about, .. }) in GAMES.iter().enumerate() {
            assert!(line(items[i].y).contains(name), "row {i} isn't {name}");
            // The description is under its name, whole when it fits.
            let below = line(items[i].y + 1);
            let shown: String = about.chars().take(30).collect();
            assert!(
                below.contains(&shown),
                "'{about}' isn't under {name}: {below}"
            );
        }
        // The rows are still where a click finds them.
        assert_eq!(row_at(&items, items[1].x + 3, items[1].y), Some(1));
    }

    #[test]
    fn games_answer_to_their_commands_names() {
        for (program, command) in [
            ("flights", Some("flights")),
            ("/usr/local/bin/jotto", Some("jotto")),
            ("WORDLE.EXE", Some("wordle")),
            ("contexto.exe", Some("contexto")),
            ("arcade", None),
            ("arcade-linux-x86_64", None),
            ("flight", None),
        ] {
            let found = game_named(OsStr::new(program)).map(|game| game.command);
            assert_eq!(found, command, "{program}");
        }
    }

    #[test]
    fn subcommands_pass_the_game_its_options() {
        let args = ArcadeArgs::try_parse_from(["arcade", "flights", "--plain", "--help"]).unwrap();
        let game = args.game.unwrap();
        assert_eq!(game.command, "flights");
        assert_eq!(game.args, ["--plain", "--help"]);
        let args = ArcadeArgs::try_parse_from(["arcade", "wordle"]).unwrap();
        assert!(args.game.unwrap().args.is_empty());
        // Plain `arcade` is the menu; `--session-saves` is for the menu alone.
        let args = ArcadeArgs::try_parse_from(["arcade", "--session-saves"]).unwrap();
        assert!(args.session_saves && args.game.is_none());
        assert!(ArcadeArgs::try_parse_from(["arcade", "--session-saves", "jotto"]).is_err());
        assert!(ArcadeArgs::try_parse_from(["arcade", "chess"]).is_err());
    }

    #[test]
    fn session_folder_is_removed() {
        let dir = SessionDir::create().unwrap();
        let path = dir.0.clone();
        fs::write(path.join("saves.json"), "{}").unwrap();
        drop(dir);
        assert!(!path.exists());
    }
}
