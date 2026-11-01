use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::process;

/// `Display` for enums with a `label()`, which is how they're shown everywhere.
macro_rules! display_label {
    ($($t:ty),*) => {$(
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.label())
            }
        }
    )*};
}

pub mod cli;
pub mod contexto;
pub mod dict;
pub mod flights;
pub mod game;
mod help;
pub mod input;
pub mod line_input;
pub mod plain;
pub mod save;
pub mod text;
pub mod tui;

use cli::CliArgs;
use game::{GameMode, GameState};
use save::Autosave;

/// Unpacks a data file `build.rs` deflated into the binary.
pub(crate) fn inflate(packed: &[u8], name: &str) -> Vec<u8> {
    miniz_oxide::inflate::decompress_to_vec(packed)
        .unwrap_or_else(|_| panic!("build.rs deflated data/{name}"))
}

/// Plays Jotto or Wordle in its TUI, resuming the save as the command does with no
/// arguments, and returns (rather than exiting on) any error: what `arcade` runs.
pub fn play(mode: GameMode) -> Result<(), String> {
    play_saved(
        mode.command_name(),
        |loaded| GameState::start(mode, None, false, None, loaded),
        tui::run,
    )
}

/// The game a command plays: its save opened by `start`, and the save kept up to
/// date as it's played.
fn open_saved<S: serde::de::DeserializeOwned, G, E: Display>(
    name: &str,
    start: impl FnOnce(save::Loaded<S>) -> Result<(G, Option<save::Notice>), E>,
) -> Result<(G, Autosave), E> {
    let path = save::save_path(name);
    let loaded = path.as_deref().map_or(save::Loaded::Missing, save::load);
    let (game, notice) = start(loaded)?;
    let mut autosave = Autosave::new(path);
    autosave.notice = notice;
    Ok((game, autosave))
}

/// Opens a game from its save with `start`, plays it with `run` and keeps the save
/// up to date, returning what went wrong instead of exiting, so a caller such as
/// `arcade` can say so and carry on.
pub(crate) fn play_saved<S: serde::de::DeserializeOwned, G, E: Display>(
    name: &str,
    start: impl FnOnce(save::Loaded<S>) -> Result<(G, Option<save::Notice>), E>,
    run: fn(G, &mut Autosave) -> io::Result<()>,
) -> Result<(), String> {
    let (game, mut autosave) = open_saved(name, start).map_err(|e| e.to_string())?;
    run(game, &mut autosave).map_err(|e| e.to_string())?;
    match autosave.error() {
        Some(err) => Err(format!("your game wasn't saved: {err}")),
        None => Ok(()),
    }
}

/// Shared CLI entrypoint for both `jotto` and `wordle` binaries.
pub fn run_cli(mode: GameMode) {
    let args = CliArgs::parse_for_mode(mode);
    run_command(
        mode.command_name(),
        "Error initializing game with custom word",
        |loaded| {
            GameState::start(
                mode,
                args.difficulty,
                args.daily,
                args.word.as_deref(),
                loaded,
            )
        },
        args.plain,
        plain::run,
        tui::run,
    );
}

/// What a game's command does: opens the game from its save with `start` (exiting
/// with `failure` and the error if it can't), then runs it in plain mode if asked
/// or when either end isn't a terminal (piped input or output), otherwise in the
/// TUI, and exits with an error message on failure. A save that failed is
/// reported once the game has closed, since the TUI would hide it.
pub(crate) fn run_command<S: serde::de::DeserializeOwned, G, E: Display>(
    name: &str,
    failure: &str,
    start: impl FnOnce(save::Loaded<S>) -> Result<(G, Option<save::Notice>), E>,
    plain: bool,
    run_plain: fn(G, &mut Autosave) -> io::Result<()>,
    run_tui: fn(G, &mut Autosave) -> io::Result<()>,
) {
    let (game, mut autosave) = open_saved(name, start).unwrap_or_else(|err| {
        eprintln!("{failure}: {err}");
        process::exit(1);
    });
    let use_plain = plain || !io::stdin().is_terminal() || !io::stdout().is_terminal();
    let res = if use_plain {
        run_plain(game, &mut autosave)
    } else {
        run_tui(game, &mut autosave)
    };
    if let Some(err) = autosave.error() {
        eprintln!("Your game wasn't saved: {err}.");
    }
    if let Err(err) = res {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}
