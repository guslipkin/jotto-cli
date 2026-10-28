use std::collections::HashSet;
use std::str::FromStr;

use jotto::dict;
use jotto::game::{
    compare_guess, score_wordle_guess, validate_word, validate_word_for_mode, Difficulty, GameMode,
    GameState, GameStatus, GuessError, LetterState, ScratchState, MAX_WORDLE_GUESSES,
};

#[test]
fn test_dictionary_populated_and_sorted_both_modes() {
    let jotto_total = dict::total_words(GameMode::Jotto);
    let wordle_total = dict::total_words(GameMode::Wordle);
    assert!(
        jotto_total >= 7000,
        "Expected at least 7000 Jotto words, got {jotto_total}"
    );
    assert!(
        wordle_total >= 11000,
        "Expected at least 11000 Wordle words, got {wordle_total}"
    );
    assert!(wordle_total > jotto_total);

    // Common 5-letter distinct-letter words must be present in both modes
    for word in ["about", "above", "abuse", "crane", "slate", "water"] {
        assert!(
            dict::contains(GameMode::Jotto, word),
            "Expected '{word}' to be in Jotto dictionary"
        );
        assert!(
            dict::contains(GameMode::Wordle, word),
            "Expected '{word}' to be in Wordle dictionary"
        );
    }

    // Words with duplicate letters must NOT be in Jotto, but MUST be in Wordle
    for dup_word in ["apple", "hello", "sheet", "vivid", "eerie"] {
        assert!(
            !dict::contains(GameMode::Jotto, dup_word),
            "Expected '{dup_word}' NOT to be in Jotto dictionary"
        );
        assert!(
            dict::contains(GameMode::Wordle, dup_word),
            "Expected '{dup_word}' to be in Wordle dictionary"
        );
    }

    // Non-words or wrong lengths must not be in either dictionary
    for non_word in ["cat", "zzzzz", "12345", "qwert"] {
        assert!(!dict::contains(GameMode::Jotto, non_word));
        assert!(!dict::contains(GameMode::Wordle, non_word));
    }
}

#[test]
fn test_difficulty_pools_are_monotonic_both_modes() {
    for mode in [GameMode::Jotto, GameMode::Wordle] {
        let easy = dict::word_count(mode, Difficulty::Easy.scowl_size());
        let medium = dict::word_count(mode, Difficulty::Medium.scowl_size());
        let hard = dict::word_count(mode, Difficulty::Hard.scowl_size());
        let scrabble = dict::word_count(mode, Difficulty::Scrabble.scowl_size());
        let insane = dict::word_count(mode, Difficulty::Insane.scowl_size());

        assert!(
            easy > 2000,
            "Easy pool for {mode:?} should have > 2000 words, got {easy}"
        );
        assert!(easy < medium);
        assert!(medium < hard);
        assert!(hard < scrabble);
        assert!(scrabble < insane);
        assert_eq!(insane, dict::total_words(mode));
    }
}

#[test]
fn test_validate_word_errors() {
    assert_eq!(validate_word("cat"), Err(GuessError::InvalidLength));
    assert_eq!(validate_word("abcdef"), Err(GuessError::InvalidLength));
    assert_eq!(validate_word("ab1de"), Err(GuessError::NonAlphabetic));
    assert_eq!(validate_word("apple"), Err(GuessError::DuplicateLetters));
    assert_eq!(validate_word("qwert"), Err(GuessError::NotInDictionary));
    assert_eq!(validate_word("about"), Ok(()));

    // Wordle allows repeating letters
    assert_eq!(validate_word_for_mode(GameMode::Wordle, "apple"), Ok(()));
    assert_eq!(validate_word_for_mode(GameMode::Wordle, "hello"), Ok(()));
    assert_eq!(
        validate_word_for_mode(GameMode::Wordle, "qwert"),
        Err(GuessError::NotInDictionary)
    );
}

#[test]
fn test_compare_guess_and_score_wordle_guess() {
    assert_eq!(compare_guess("above", "about"), 3); // a, b, o
    assert_eq!(compare_guess("slate", "least"), 5); // anagram: all 5 in common
    assert_eq!(compare_guess("brick", "jumpy"), 0); // 0 in common
    assert_eq!(compare_guess("crane", "slate"), 2); // a, e

    // Wordle exact/present/absent with duplicate letters:
    // Secret: "apple" (a=0, p=1, p=2, l=3, e=4)
    // Guess:  "puppy" -> p(0)=Present(matches p@1), u(1)=Absent, p(2)=Correct(matches p@2), p(3)=Absent(no 3rd p!), y(4)=Absent
    assert_eq!(
        score_wordle_guess("apple", "puppy"),
        [
            LetterState::Present,
            LetterState::Absent,
            LetterState::Correct,
            LetterState::Absent,
            LetterState::Absent,
        ]
    );

    // Secret: "crane", Guess: "eerie"
    // Only one 'e' in "crane" at index 4 (exact match for guess[4]), so guess[0] and guess[1] must be Absent!
    assert_eq!(
        score_wordle_guess("crane", "eerie"),
        [
            LetterState::Absent,
            LetterState::Absent,
            LetterState::Present,
            LetterState::Absent,
            LetterState::Correct,
        ]
    );
}

#[test]
fn test_jotto_game_state_flow() {
    let mut game = GameState::new(Difficulty::Easy, Some("above")).expect("valid custom word");
    assert_eq!(game.mode(), GameMode::Jotto);
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

    // Resetting game starts fresh
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
fn test_wordle_game_state_and_six_guess_limit() {
    let mut game = GameState::new_with_mode(GameMode::Wordle, Difficulty::Scrabble, Some("apple"))
        .expect("valid wordle secret with repeating letters");
    assert_eq!(game.mode(), GameMode::Wordle);
    assert_eq!(game.max_guesses(), Some(MAX_WORDLE_GUESSES));

    let wrong_guesses = ["crane", "slate", "brick", "jumpy", "ghost", "vivid"];
    for (idx, &g) in wrong_guesses.iter().enumerate() {
        let r = game.make_guess(g).expect("valid wordle guess");
        assert_eq!(r.round, idx + 1);
        if idx + 1 < MAX_WORDLE_GUESSES {
            assert_eq!(game.status(), GameStatus::Playing);
        } else {
            assert_eq!(game.status(), GameStatus::Lost);
        }
    }

    // Letter states should upgrade monotonically (Absent -> Present -> Correct)
    // 'a' was in "crane" (idx 2 = Present) and "slate" (idx 2 = Present)
    assert_eq!(game.letter_states()[0], Some(LetterState::Present));
    // 'e' was in "crane" (idx 4 = Correct for "apple")
    assert_eq!(game.letter_states()[4], Some(LetterState::Correct));
    // 'c' was in "crane" (Absent)
    assert_eq!(game.letter_states()[2], Some(LetterState::Absent));

    // Calling make_guess after the game is over must return Err(GuessError::GameOver)
    assert_eq!(game.make_guess("apple"), Err(GuessError::GameOver));

    // Shareable summary grid should reflect X/6 and 6 rows of emoji
    let summary = game.wordle_share_summary();
    assert!(summary.starts_with("Wordle (Scrabble (80)) X/6\n"));
    assert_eq!(summary.lines().count(), 7);
}

#[test]
fn test_random_mode_never_repeats_until_pool_exhausted() {
    let mut game =
        GameState::new_with_mode(GameMode::Wordle, Difficulty::Easy, None).expect("new game");
    let mut seen = HashSet::new();
    seen.insert(game.secret_word().to_string());

    for _ in 1..200 {
        game.reset(None);
        assert!(
            seen.insert(game.secret_word().to_string()),
            "Random game repeated word before pool was exhausted"
        );
    }
    assert_eq!(game.played_words_count(), 200);
}

#[test]
fn test_give_up_and_difficulty_parsing() {
    let mut game = GameState::new(Difficulty::Scrabble, None).expect("new game");
    game.give_up();
    assert_eq!(game.status(), GameStatus::GivenUp);
    assert_eq!(game.make_guess("about"), Err(GuessError::GameOver));

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
fn test_daily_mode_determinism_and_cooldown_both_modes() {
    assert_eq!(dict::format_utc_date(0), "1970-01-01");
    assert_eq!(dict::format_utc_date(20_754), "2026-10-28");

    for mode in [GameMode::Jotto, GameMode::Wordle] {
        let w1 = dict::daily_word_for_day(mode, 20_754);
        let w2 = dict::daily_word_for_day(mode, 20_754);
        assert_eq!(
            w1, w2,
            "Daily word must be deterministic for the same UTC day"
        );
        assert!(dict::contains(mode, w1));
        assert!(
            dict::word_difficulty(mode, w1).unwrap() <= Difficulty::Scrabble.scowl_size(),
            "Daily word must come from the Scrabble (80) word list"
        );

        // Verify no word repeats across 1,000 consecutive days
        let mut seen = HashSet::new();
        let base_day = 20_000u64;
        for offset in 0..1_000 {
            let word = dict::daily_word_for_day(mode, base_day + offset);
            assert!(
                seen.insert(word),
                "Daily word '{word}' for {mode:?} repeated within cooldown window at offset {offset}"
            );
        }

        let mut daily_game = GameState::new_daily_with_mode(mode);
        assert!(daily_game.is_daily());
        assert_eq!(daily_game.mode(), mode);
        assert_eq!(daily_game.difficulty(), Difficulty::Scrabble);
        assert!(daily_game.daily_date().is_some());
        assert_eq!(daily_game.secret_word().len(), 5);
        assert_eq!(daily_game.played_words_count(), 1);

        // Switching from a random game to start_daily() should also record the daily word in played_words
        daily_game.reset(None);
        let count_after_reset = daily_game.played_words_count();
        assert!(count_after_reset >= 1);
        daily_game.start_daily();
        assert!(daily_game.played_words_count() >= count_after_reset);
    }
}
