use std::io::{self, BufRead, Write};
use std::str::FromStr;

use crate::dict;
use crate::game::{Difficulty, GameState, GameStatus, ScratchState};

/// Runs the line-by-line REPL / Plain CLI mode.
pub fn run(mut game: GameState) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    if let Some(date) = game.daily_date() {
        println!(
            "=== JOTTO [DAILY {date} UTC] === (Difficulty: {} | {} words)",
            game.difficulty(),
            dict::word_count(game.difficulty().scowl_size())
        );
    } else {
        println!(
            "=== JOTTO === (Difficulty: {} | {} words)",
            game.difficulty(),
            dict::word_count(game.difficulty().scowl_size())
        );
    }
    println!("Type a 5-letter word to guess, or ':help' for commands.");

    let mut lines = stdin.lock().lines();
    loop {
        print!("jotto> ");
        stdout.flush()?;

        let Some(line_res) = lines.next() else {
            println!();
            break;
        };
        let raw_line = line_res?;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let lower = trimmed.to_ascii_lowercase();
        let mut parts = lower.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        let arg = rest.first().copied();

        match cmd {
            ":quit" | "quit" | ":exit" | "exit" | ":q" => {
                break;
            }
            ":help" | "help" | "?" => {
                print_help();
                continue;
            }
            ":credits" | "credits" => {
                print_credits();
                continue;
            }
            ":history" | "history" => {
                print_board(&game);
                continue;
            }
            ":scratch" | "scratch" => {
                for token in &rest {
                    for ch in token.chars() {
                        if ch.is_ascii_alphabetic() {
                            game.toggle_scratch(ch);
                        }
                    }
                }
                print_scratch(&game);
                continue;
            }
            ":giveup" | "giveup" => {
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
            "give" if arg == Some("up") => {
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
                    dict::word_count(game.difficulty().scowl_size())
                );
                continue;
            }
            ":daily" | "daily" => {
                game.start_daily();
                let date_str = game.daily_date().unwrap_or("today");
                println!(
                    "Daily Challenge ({date_str} UTC) started! Difficulty: {} ({} words).",
                    game.difficulty(),
                    dict::word_count(game.difficulty().scowl_size())
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
                                dict::word_count(game.difficulty().scowl_size())
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
                    println!(
                        "Already guessed '{}' in Round {} ({} {} in common).",
                        round.guess.to_ascii_uppercase(),
                        round.round,
                        round.common,
                        if round.common == 1 {
                            "letter"
                        } else {
                            "letters"
                        }
                    );
                    continue;
                }
                print_board(&game);
                if game.status() == GameStatus::Won {
                    let count = game.history().len();
                    println!(
                        "CONGRATULATIONS!!! You guessed {} in {} {}.",
                        game.secret_word().to_ascii_uppercase(),
                        count,
                        if count == 1 { "guess" } else { "guesses" }
                    );
                }
            }
            Err(err) => {
                println!("{err}");
            }
        }
    }

    Ok(())
}

fn format_scratch_tokens(game: &GameState) -> Vec<String> {
    game.scratched_letters()
        .iter()
        .enumerate()
        .filter_map(|(i, &state)| {
            let ch = (b'A' + i as u8) as char;
            match state {
                ScratchState::Default => None,
                ScratchState::ScratchedOut => Some(format!("~{ch}~")),
                ScratchState::Positive => Some(format!("+{ch}+")),
            }
        })
        .collect()
}

fn print_scratch(game: &GameState) {
    let tokens = format_scratch_tokens(game);
    if tokens.is_empty() {
        println!("Scratch: (none)");
    } else {
        println!("Scratch: {}", tokens.join(" "));
    }
}

fn print_board(game: &GameState) {
    println!();
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

    let scratch = format_scratch_tokens(game);
    if !scratch.is_empty() {
        println!("Scratch:      {}", scratch.join(" "));
    }
    println!();
}

fn print_help() {
    println!();
    println!("Jotto is a game where you try to guess the five-letter word:");
    println!("  - The word will have no repeating letters");
    println!("  - The word is not a proper noun");
    println!("  - You are only told how many letters your guess has in common with the word");
    println!();
    println!("Difficulty Levels (each includes words from easier levels):");
    println!("  - easy (35):     Recommended small dictionary");
    println!("  - medium (50):   Recommended medium dictionary");
    println!("  - hard (70):     Recommended large dictionary");
    println!("  - scrabble (80): Strange and unusual word-game words (default)");
    println!("  - insane (95):   Every English word in SCOWL");
    println!();
    println!("Commands:");
    println!("  :help                Show this help message");
    println!("  :history             Show guess history and used letters");
    println!("  :scratch [letters]   Toggle scratch state (~X~ -> +X+ -> default) for letters");
    println!("  :new [difficulty]    Start a new random game");
    println!("  :daily [difficulty]  Start today's deterministic UTC daily challenge");
    println!("  :difficulty [level]  Show or change difficulty");
    println!("  :giveup              Reveal the secret word");
    println!("  :credits             Show credits");
    println!("  :quit                Exit Jotto");
    println!();
}

fn print_credits() {
    println!();
    println!("Jotto CLI was written by Gus Lipkin (https://guslipkin.me).");
    println!("The word list comes from SCOWL (http://wordlist.aspell.net/).");
    println!();
}
