use std::io;
use std::str::FromStr;

use crate::dict;
use crate::game::{
    supports_color, Difficulty, GameMode, GameState, GameStatus, LetterState, ScratchState,
    MAX_WORDLE_GUESSES,
};
use crate::line_input::LineReader;
use crate::save::Autosave;
use crate::text::AUTHOR;
use crate::tui::{ansi_rgb, COLOR_NEGATIVE, COLOR_POSITIVE, COLOR_WARNING};

/// A game's plain-mode title, `=== TITLE ===`, with the date for a daily game.
pub(crate) fn title_line(title: &str, daily: Option<&str>) -> String {
    match daily {
        Some(date) => format!("=== {title} [DAILY {date} UTC] ==="),
        None => format!("=== {title} ==="),
    }
}

/// Saves the game (every command and guess comes back here, so this saves each
/// change), then reads the next line that isn't blank, trimmed and lower-cased:
/// `None` once input ends.
pub(crate) fn next_line(
    reader: &mut LineReader,
    autosave: &mut Autosave,
    saved: &impl serde::Serialize,
    prompt: &str,
) -> io::Result<Option<String>> {
    autosave.sync(saved);
    loop {
        let Some(raw_line) = reader.read_line(prompt)? else {
            println!();
            return Ok(None);
        };
        let lower = raw_line.trim().to_ascii_lowercase();
        if !lower.is_empty() {
            return Ok(Some(lower));
        }
    }
}

/// A command line's command and, after the first space, its argument.
pub(crate) fn split_command(line: &str) -> (&str, Option<&str>) {
    match line.split_once(char::is_whitespace) {
        Some((cmd, arg)) => (cmd, Some(arg.trim())),
        None => (line, None),
    }
}

/// Runs the line-by-line REPL / Plain CLI mode for either Jotto or Wordle.
pub fn run(mut game: GameState, autosave: &mut Autosave) -> io::Result<()> {
    let mode = game.mode();
    let use_color = supports_color();

    println!(
        "{} (Difficulty: {} | {} words)",
        title_line(mode.title(), game.daily_date()),
        game.difficulty(),
        dict::word_count(mode, game.difficulty().scowl_size())
    );
    println!("Type a 5-letter word to guess, or ':help' for commands.");
    if let Some(notice) = autosave.notice.take() {
        println!("{}", notice.text());
        if !game.history().is_empty() {
            print_board(&game, use_color);
        }
    }

    let mut reader = LineReader::new();
    let prompt = format!("{}> ", mode.command_name());
    while let Some(lower) = next_line(&mut reader, autosave, &game.to_saved(), &prompt)? {
        let mut parts = lower.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        let arg = rest.first().copied();

        match cmd {
            ":quit" | "quit" | ":exit" | "exit" | ":q" => {
                break;
            }
            ":help" | "help" | "?" => {
                print_help(mode, use_color);
                continue;
            }
            ":credits" | "credits" => {
                print_credits(mode);
                continue;
            }
            ":history" | "history" => {
                print_board(&game, use_color);
                continue;
            }
            // Jotto only: Wordle has no scratch pad, so there it's an unknown command.
            ":scratch" | "scratch" if mode == GameMode::Jotto => {
                for token in &rest {
                    for ch in token.chars() {
                        if ch.is_ascii_alphabetic() {
                            game.toggle_scratch(ch);
                        }
                    }
                }
                print_scratch(&game, use_color);
                continue;
            }
            // "give up" as two words too.
            ":giveup" | "giveup" | "give" if cmd != "give" || arg == Some("up") => {
                if game.status() != GameStatus::Playing {
                    println!(
                        "The current game is already over. Type ':new' or ':daily' to play again."
                    );
                } else {
                    game.give_up();
                    println!("Your word is: {}", game.secret_word().to_ascii_uppercase());
                }
                continue;
            }
            ":new" | "new" | ":reset" | "reset" => {
                if let Some(diff_str) = arg {
                    match Difficulty::from_str(diff_str) {
                        Ok(d) => game.reset(Some(d)),
                        Err(e) => {
                            println!("{e}");
                            continue;
                        }
                    }
                } else {
                    game.reset(None);
                }
                println!(
                    "New game started! Difficulty: {} ({} words).",
                    game.difficulty(),
                    dict::word_count(mode, game.difficulty().scowl_size())
                );
                continue;
            }
            ":daily" | "daily" => {
                game.start_daily();
                let date_str = game.daily_date().unwrap_or("today");
                println!(
                    "Daily Challenge ({date_str} UTC) started! Difficulty: {} ({} words).",
                    game.difficulty(),
                    dict::word_count(mode, game.difficulty().scowl_size())
                );
                continue;
            }
            ":difficulty" | "difficulty" => {
                if let Some(diff_str) = arg {
                    match Difficulty::from_str(diff_str) {
                        Ok(d) => {
                            game.reset(Some(d));
                            println!(
                                "Difficulty set to {} ({} words). New game started!",
                                game.difficulty(),
                                dict::word_count(mode, game.difficulty().scowl_size())
                            );
                        }
                        Err(e) => println!("{e}"),
                    }
                } else {
                    println!("Current difficulty: {}", game.difficulty());
                }
                continue;
            }
            _ => {}
        }

        if game.status() != GameStatus::Playing {
            println!(
                "The current game is over. Type ':new' to start a new game or ':quit' to exit."
            );
            continue;
        }

        let prev_len = game.history().len();
        match game.make_guess(&lower) {
            Ok(round) => {
                if game.history().len() == prev_len {
                    match mode {
                        GameMode::Jotto => println!(
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
                        GameMode::Wordle => println!(
                            "Already guessed '{}' in Guess {}.",
                            round.guess.to_ascii_uppercase(),
                            round.round
                        ),
                    }
                    continue;
                }
                print_board(&game, use_color);
                match game.status() {
                    GameStatus::Won => {
                        let count = game.history().len();
                        println!(
                            "CONGRATULATIONS!!! You guessed {} in {} {}.",
                            game.secret_word().to_ascii_uppercase(),
                            count,
                            if count == 1 { "guess" } else { "guesses" }
                        );
                        if mode == GameMode::Wordle {
                            println!("\n{}\n", game.wordle_share_summary());
                        }
                    }
                    GameStatus::Lost => {
                        println!(
                            "Out of guesses! Your word was: {}",
                            game.secret_word().to_ascii_uppercase()
                        );
                        if mode == GameMode::Wordle {
                            println!("\n{}\n", game.wordle_share_summary());
                        }
                    }
                    GameStatus::Playing | GameStatus::GivenUp => {}
                }
            }
            Err(err) => {
                println!("{err}");
            }
        }
    }

    Ok(())
}

fn format_wordle_letter(ch: char, state: LetterState, use_color: bool, padded: bool) -> String {
    if use_color {
        match state {
            LetterState::Correct => {
                format!("\x1b[1;30;48;2;{}m {ch} \x1b[0m", ansi_rgb(COLOR_POSITIVE))
            }
            LetterState::Present => {
                format!("\x1b[1;30;48;2;{}m {ch} \x1b[0m", ansi_rgb(COLOR_WARNING))
            }
            LetterState::Absent => {
                if padded {
                    format!("\x1b[90;9m {ch} \x1b[0m")
                } else {
                    format!("\x1b[90;9m{ch}\x1b[0m")
                }
            }
        }
    } else {
        match state {
            LetterState::Correct => format!("+{ch}+"),
            LetterState::Present => format!("?{ch}?"),
            LetterState::Absent => format!("~{ch}~"),
        }
    }
}

/// Every letter of the scratch pad, A to Z, each three cells wide: scratched out
/// and positive ones styled, the rest plain, so the whole alphabet is shown and
/// the unmarked letters can be read off at a glance (issue #2).
fn format_scratch_tokens(game: &GameState, use_color: bool) -> Vec<String> {
    game.scratched_letters()
        .iter()
        .enumerate()
        .map(|(i, &state)| {
            let ch = (b'A' + i as u8) as char;
            match (state, use_color) {
                (ScratchState::Default, _) => format!(" {ch} "),
                (ScratchState::ScratchedOut, true) => format!(
                    "\x1b[1;9;30;48;2;{}m {ch} \x1b[0m",
                    ansi_rgb(COLOR_NEGATIVE)
                ),
                (ScratchState::ScratchedOut, false) => format!("~{ch}~"),
                (ScratchState::Positive, true) => {
                    format!("\x1b[1;30;48;2;{}m {ch} \x1b[0m", ansi_rgb(COLOR_POSITIVE))
                }
                (ScratchState::Positive, false) => format!("+{ch}+"),
            }
        })
        .collect()
}

/// Cells a scratch letter takes, and the space between letters.
const SCRATCH_CELL: usize = 3;
const SCRATCH_GAP: usize = 1;

/// The scratch pad as lines for a terminal `width` cells wide, starting with
/// `label` and indenting the rest under it. The letters are split into even rows
/// that fit, rather than one long line the terminal wraps wherever it ends,
/// mid-letter, with the styled letters' colours spilling over (issue #3).
fn scratch_lines(tokens: &[String], label: &str, width: usize) -> Vec<String> {
    let room = width.saturating_sub(label.chars().count());
    let fit = ((room + SCRATCH_GAP) / (SCRATCH_CELL + SCRATCH_GAP)).max(1);
    // As few rows as fit, as evenly filled as they can be (A-M and N-Z at 80).
    let rows = tokens.len().div_ceil(fit).max(1);
    let per_row = tokens.len().div_ceil(rows).max(1);
    let indent = " ".repeat(label.chars().count());
    tokens
        .chunks(per_row)
        .enumerate()
        .map(|(i, row)| {
            let lead = if i == 0 { label } else { indent.as_str() };
            format!("{lead}{}", row.join(" "))
        })
        .collect()
}

/// The terminal's width, or 80 when it can't be told (piped output, say).
fn terminal_width() -> usize {
    crossterm::terminal::size().map_or(80, |(w, _)| usize::from(w).max(1))
}

fn print_scratch(game: &GameState, use_color: bool) {
    let tokens = format_scratch_tokens(game, use_color);
    for line in scratch_lines(&tokens, "Scratch: ", terminal_width()) {
        println!("{line}");
    }
}

fn print_board(game: &GameState, use_color: bool) {
    println!();
    match game.mode() {
        GameMode::Jotto => {
            println!("+-------+---------+--------+");
            println!("| Round |  Guess  | Common |");
            println!("+-------+---------+--------+");
            for r in game.history().iter().rev() {
                println!(
                    "| {:>5} |  {}  | {:>6} |",
                    r.round,
                    r.guess.to_ascii_uppercase(),
                    r.common
                );
            }
            println!("+-------+---------+--------+");

            let used: Vec<String> = game
                .used_letters()
                .iter()
                .enumerate()
                .filter_map(|(i, &u)| {
                    if u {
                        Some(((b'A' + i as u8) as char).to_string())
                    } else {
                        None
                    }
                })
                .collect();

            if used.is_empty() {
                println!("Used Letters: (none)");
            } else {
                println!("Used Letters: {}", used.join(" "));
            }

            let scratch = format_scratch_tokens(game, use_color);
            for line in scratch_lines(&scratch, "Scratch:      ", terminal_width()) {
                println!("{line}");
            }
        }
        GameMode::Wordle => {
            println!("+-------+-----------------------+");
            println!("| Guess |         Board         |");
            println!("+-------+-----------------------+");
            for r in game.history() {
                let tiles: Vec<String> = r
                    .guess
                    .chars()
                    .map(|c| c.to_ascii_uppercase())
                    .zip(r.letter_states.iter())
                    .map(|(ch, &state)| format_wordle_letter(ch, state, use_color, true))
                    .collect();
                println!(
                    "| {:>3}/{} |  {}  |",
                    r.round,
                    MAX_WORDLE_GUESSES,
                    tiles.join(" ")
                );
            }
            println!("+-------+-----------------------+");

            let alphabet_tokens: Vec<String> = game
                .letter_states()
                .iter()
                .enumerate()
                .map(|(i, &state)| {
                    let ch = (b'A' + i as u8) as char;
                    match state {
                        Some(s) => format_wordle_letter(ch, s, use_color, false),
                        None => ch.to_string(),
                    }
                })
                .collect();

            println!("Used Letters: {}", alphabet_tokens.join(" "));
        }
    }
    println!();
}

fn print_help(mode: GameMode, use_color: bool) {
    let easy_n = dict::word_count(mode, Difficulty::Easy.scowl_size());
    let med_n = dict::word_count(mode, Difficulty::Medium.scowl_size());
    let hard_n = dict::word_count(mode, Difficulty::Hard.scowl_size());
    let scrab_n = dict::word_count(mode, Difficulty::Scrabble.scowl_size());
    let insane_n = dict::word_count(mode, Difficulty::Insane.scowl_size());

    println!();
    match mode {
        GameMode::Jotto => {
            println!("Jotto is a game where you try to guess the five-letter word:");
            println!("  - The word will have no repeating letters");
            println!("  - The word is not a proper noun");
            println!(
                "  - You are only told how many letters your guess has in common with the word"
            );
        }
        GameMode::Wordle => {
            let exact = format_wordle_letter('X', LetterState::Correct, use_color, false);
            let present = format_wordle_letter('X', LetterState::Present, use_color, false);
            let absent = format_wordle_letter('X', LetterState::Absent, use_color, false);
            println!("Wordle is a game where you have 6 guesses to find the five-letter word:");
            println!("  - Repeating letters are allowed; the word is not a proper noun or plural");
            println!("  - {exact} means letter X is in the word and in the exact spot");
            println!("  - {present} means letter X is in the word, but in a different spot");
            println!("  - {absent} means letter X is not in the word in any spot");
        }
    }
    println!();
    println!("Difficulty Levels (each includes words from easier levels):");
    println!("  - easy (35):     Recommended small dictionary ({easy_n} words)");
    println!("  - medium (50):   Recommended medium dictionary ({med_n} words)");
    println!("  - hard (70):     Recommended large dictionary ({hard_n} words)");
    println!("  - scrabble (80): Strange and unusual word-game words ({scrab_n} words, default)");
    println!("  - insane (95):   Every English word in SCOWL ({insane_n} words)");
    println!();
    println!("Commands:");
    println!("  :help                Show this help message");
    println!("  :history             Show guess history and used letters");
    if mode == GameMode::Jotto {
        println!(
            "  :scratch [letters]   Toggle scratch state (scratched -> positive -> default) for letters"
        );
    }
    println!("  :new [difficulty]    Start a new random game");
    println!("  :daily               Start today's deterministic UTC daily challenge");
    println!("  :difficulty [level]  Show or change difficulty");
    println!("  :giveup              Reveal the secret word");
    println!("  :credits             Show credits");
    println!("  :quit                Exit {}", mode.title());
    println!();
}

fn print_credits(mode: GameMode) {
    println!();
    println!("{} CLI was written by {AUTHOR}.", mode.title());
    println!("The word list comes from SCOWL (http://wordlist.aspell.net/).");
    if mode == GameMode::Wordle {
        println!(
            "Wordle was created by Josh Wardle; play the original at https://www.nytimes.com/games/wordle"
        );
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_tokens() -> Vec<String> {
        (b'A'..=b'Z').map(|c| format!(" {} ", c as char)).collect()
    }

    #[test]
    fn scratch_shows_every_letter() {
        let mut game = GameState::new_with_mode(GameMode::Jotto, Difficulty::Easy, None).unwrap();
        game.toggle_scratch('e');
        game.toggle_scratch('q');
        game.toggle_scratch('q');
        let tokens = format_scratch_tokens(&game, false);
        assert_eq!(tokens.len(), 26);
        assert_eq!(tokens[0], " A ");
        assert_eq!(tokens[4], "~E~");
        assert_eq!(tokens[16], "+Q+");
    }

    #[test]
    fn scratch_rows_fit_the_terminal() {
        let tokens = plain_tokens();
        for width in [40, 60, 80, 120, 200] {
            let lines = scratch_lines(&tokens, "Scratch: ", width);
            for line in &lines {
                assert!(
                    line.chars().count() <= width,
                    "{width}: '{line}' is too wide"
                );
            }
            // Every letter, once, in order.
            let letters: String = lines
                .iter()
                .flat_map(|l| l.chars().filter(char::is_ascii_uppercase))
                .collect::<String>()
                .replace("S", "")
                .chars()
                .collect();
            assert!(
                letters.ends_with("ABCDEFGHIJKLMNOPQRTUVWXYZ"),
                "{width}: {letters}"
            );
        }
        // At 80 columns: A-M, then N-Z under it.
        let lines = scratch_lines(&tokens, "Scratch: ", 80);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].trim_end().ends_with('M'));
        assert!(lines[1].starts_with("          N "), "{:?}", lines[1]);
        // Wide enough, one line.
        assert_eq!(scratch_lines(&tokens, "Scratch: ", 120).len(), 1);
    }
}
