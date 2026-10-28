use std::str::FromStr;

use jotto::dict;
use jotto::game::{
    compare_guess, validate_word, Difficulty, GameState, GameStatus, GuessError, ScratchState,
};

#[test]
fn test_dictionary_populated_and_sorted() {
    let total = dict::total_words();
    assert!(total >= 7000, "Expected at least 7000 words, got {total}");

    // Common 5-letter distinct-letter words must be present
    for word in ["about", "above", "abuse", "crane", "slate", "water"] {
        assert!(
            dict::contains(word),
            "Expected '{word}' to be in dictionary"
        );
    }

    // Words with duplicate letters or wrong length must not be in dictionary
    for non_word in ["apple", "hello", "sheet", "cat", "zzzzz", "12345"] {
        assert!(
            !dict::contains(non_word),
            "Expected '{non_word}' NOT to be in dictionary"
        );
    }
}

#[test]
fn test_difficulty_pools_are_monotonic() {
    let easy = dict::word_count(Difficulty::Easy.scowl_size());
    let medium = dict::word_count(Difficulty::Medium.scowl_size());
    let hard = dict::word_count(Difficulty::Hard.scowl_size());
    let scrabble = dict::word_count(Difficulty::Scrabble.scowl_size());
    let insane = dict::word_count(Difficulty::Insane.scowl_size());

    assert!(
        easy > 2000,
        "Easy pool should have > 2000 words, got {easy}"
    );
    assert!(easy < medium);
    assert!(medium < hard);
    assert!(hard < scrabble);
    assert!(scrabble < insane);
    assert!(
        insane >= 7000,
        "Insane pool should have >= 7000 words, got {insane}"
    );
    assert_eq!(insane, dict::total_words());
}

#[test]
fn test_validate_word_errors() {
    assert_eq!(validate_word("cat"), Err(GuessError::InvalidLength));
    assert_eq!(validate_word("abcdef"), Err(GuessError::InvalidLength));
    assert_eq!(validate_word("ab1de"), Err(GuessError::NonAlphabetic));
    assert_eq!(validate_word("apple"), Err(GuessError::DuplicateLetters));
    assert_eq!(validate_word("qwert"), Err(GuessError::NotInDictionary));
    assert_eq!(validate_word("about"), Ok(()));
}

#[test]
fn test_compare_guess() {
    assert_eq!(compare_guess("above", "about"), 3); // a, b, o
    assert_eq!(compare_guess("slate", "least"), 5); // anagram: all 5 in common
    assert_eq!(compare_guess("brick", "jumpy"), 0); // 0 in common
    assert_eq!(compare_guess("crane", "slate"), 2); // a, e
}

#[test]
fn test_game_state_flow() {
    let mut game = GameState::new(Difficulty::Easy, Some("above")).expect("valid custom word");
    assert_eq!(game.status(), GameStatus::Playing);
    assert_eq!(game.secret_word(), "above");

    // Round 1: guess "about" -> 3 in common (a, b, o)
    let r1 = game.make_guess("ABOUT").expect("valid guess");
    assert_eq!(r1.round, 1);
    assert_eq!(r1.guess, "about");
    assert_eq!(r1.common, 3);
    assert_eq!(game.status(), GameStatus::Playing);

    // Used letters should mark a, b, o, u, t
    let used = game.used_letters();
    assert!(used[0]); // a
    assert!(used[1]); // b
    assert!(!used[2]); // c

    // Repeating same guess doesn't add a duplicate history row
    let r1_dup = game.make_guess("about").expect("valid guess");
    assert_eq!(r1_dup.round, 1);
    assert_eq!(game.history().len(), 1);

    // Winning guess
    let r2 = game.make_guess("above").expect("valid winning guess");
    assert_eq!(r2.round, 2);
    assert_eq!(r2.common, 5);
    assert_eq!(game.status(), GameStatus::Won);

    // Scratchpad 3-state cycle: Default -> ScratchedOut -> Positive -> Default
    assert_eq!(game.scratched_letters()[23], ScratchState::Default);
    game.toggle_scratch('x');
    assert_eq!(game.scratched_letters()[23], ScratchState::ScratchedOut);
    game.toggle_scratch('X');
    assert_eq!(game.scratched_letters()[23], ScratchState::Positive);
    game.toggle_scratch('x');
    assert_eq!(game.scratched_letters()[23], ScratchState::Default);

    game.toggle_scratch('z');
    assert_eq!(game.scratched_letters()[25], ScratchState::ScratchedOut);

    // Resetting game starts fresh (including scratchpad)
    game.reset(Some(Difficulty::Hard));
    assert_eq!(game.status(), GameStatus::Playing);
    assert_eq!(game.difficulty(), Difficulty::Hard);
    assert!(game.history().is_empty());
    assert!(game.used_letters().iter().all(|&u| !u));
    assert!(game
        .scratched_letters()
        .iter()
        .all(|&s| s == ScratchState::Default));
}

#[test]
fn test_give_up_and_difficulty_parsing() {
    let mut game = GameState::new(Difficulty::Scrabble, None).expect("new game");
    game.give_up();
    assert_eq!(game.status(), GameStatus::GivenUp);

    assert_eq!(Difficulty::from_str("easy"), Ok(Difficulty::Easy));
    assert_eq!(Difficulty::from_str("35"), Ok(Difficulty::Easy));
    assert_eq!(Difficulty::from_str("medium"), Ok(Difficulty::Medium));
    assert_eq!(Difficulty::from_str("hard"), Ok(Difficulty::Hard));
    assert_eq!(Difficulty::from_str("scrabble"), Ok(Difficulty::Scrabble));
    assert_eq!(Difficulty::from_str("insane"), Ok(Difficulty::Insane));
    assert_eq!(Difficulty::from_str("95"), Ok(Difficulty::Insane));
    assert!(Difficulty::from_str("invalid").is_err());
}

#[test]
fn test_daily_mode_determinism_and_cooldown() {
    // 1970-01-01 is day 0; 2026-10-28 is day 20754
    assert_eq!(dict::format_utc_date(0), "1970-01-01");
    assert_eq!(dict::format_utc_date(20_754), "2026-10-28");

    let w1 = dict::daily_word_for_day(20_754);
    let w2 = dict::daily_word_for_day(20_754);
    assert_eq!(
        w1, w2,
        "Daily word must be deterministic for the same UTC day"
    );
    assert!(dict::contains(w1));
    assert!(
        dict::word_difficulty(w1).unwrap() <= Difficulty::Scrabble.scowl_size(),
        "Daily word must come from the Scrabble (80) word list"
    );

    // Verify no word repeats across a multi-year window (e.g. 1,000 consecutive days)
    let mut seen = std::collections::HashSet::new();
    let base_day = 20_000u64;
    for offset in 0..1_000 {
        let word = dict::daily_word_for_day(base_day + offset);
        assert!(
            seen.insert(word),
            "Daily word '{word}' repeated within cooldown window at offset {offset}"
        );
    }

    let daily_game = GameState::new_daily();
    assert!(daily_game.is_daily());
    assert_eq!(daily_game.difficulty(), Difficulty::Scrabble);
    assert!(daily_game.daily_date().is_some());
    assert_eq!(daily_game.secret_word().len(), 5);
}
