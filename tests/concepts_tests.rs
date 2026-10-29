use std::collections::HashSet;

use jotto::concepts::data::lexicon;
use jotto::concepts::game::{
    ConceptsGame, Difficulty, GuessError, HintError, Status, DAILY_DIFFICULTY, FIRST_HINT_RANK,
};
use jotto::dict::DAILY_EPOCH_DAY;

#[test]
fn test_lexicon_populated_and_sorted() {
    let lex = lexicon();
    assert!(
        lex.ranked_count() >= 20_000,
        "only {} ranked words",
        lex.ranked_count()
    );
    assert!(
        lex.guessable_count() >= lex.ranked_count() + 3_000,
        "only {} guessable words",
        lex.guessable_count()
    );
    assert!(
        Difficulty::Hard.secret_count() >= 2_000,
        "only {} secrets",
        Difficulty::Hard.secret_count()
    );
    for i in 1..lex.guessable_count() {
        assert!(lex.word(i - 1) < lex.word(i), "words aren't sorted at {i}");
    }
    for word in ["dog", "ocean", "guitar", "run", "happy", "quickly"] {
        assert!(lex.index_of(word).is_some(), "'{word}' should be guessable");
    }
    // Function words and names aren't guessable.
    for word in ["the", "of", "and", "she", "london"] {
        assert!(
            lex.resolve(word).is_none(),
            "'{word}' shouldn't be guessable"
        );
    }
}

#[test]
fn test_inflections_resolve_to_base_form() {
    let lex = lexicon();
    for (form, base) in [
        ("cats", "cat"),
        ("mice", "mouse"),
        ("children", "child"),
        ("went", "go"),
        ("ran", "run"),
        ("happier", "happy"),
    ] {
        let idx = lex
            .resolve(form)
            .unwrap_or_else(|| panic!("'{form}' didn't resolve"));
        assert_eq!(lex.word(idx), base, "'{form}' resolved to the wrong word");
    }
}

#[test]
fn test_secrets_are_guessable_nouns_and_unique() {
    let lex = lexicon();
    let mut seen = HashSet::new();
    for idx in lex.secret_pool(3) {
        assert!(lex.is_ranked(idx));
        assert!(
            seen.insert(idx),
            "secret '{}' is listed twice",
            lex.word(idx)
        );
    }
    for not_secret in ["run", "happy", "quickly", "two", "kill"] {
        let idx = lex.index_of(not_secret).unwrap();
        assert_eq!(
            lex.secret_tier(idx),
            None,
            "'{not_secret}' shouldn't be a secret"
        );
    }
}

#[test]
fn test_ranking_is_a_permutation_with_secret_first() {
    let lex = lexicon();
    let secret = lex.index_of("ocean").unwrap();
    let ranking = lex.rank_against(secret);
    assert_eq!(ranking.rank_of(secret), 1);
    assert_eq!(ranking.word_at(1), secret);
    let mut ranks: Vec<usize> = (0..lex.guessable_count())
        .filter(|&i| lex.is_ranked(i))
        .map(|i| ranking.rank_of(i))
        .collect();
    ranks.sort_unstable();
    assert!(ranks.iter().enumerate().all(|(i, &r)| r == i + 1));
    for rank in [1, 2, 100, lex.ranked_count()] {
        assert_eq!(ranking.rank_of(ranking.word_at(rank)), rank);
    }
    // Related words rank well above unrelated ones.
    let sea = ranking.rank_of(lex.index_of("sea").unwrap());
    let violin = ranking.rank_of(lex.index_of("violin").unwrap());
    assert!(sea <= 10, "'sea' is #{sea} for 'ocean'");
    assert!(violin > 1_000, "'violin' is #{violin} for 'ocean'");
}

#[test]
fn test_guess_only_words_share_a_ranked_words_rank() {
    let lex = lexicon();
    let secret = lex.index_of("ocean").unwrap();
    let ranking = lex.rank_against(secret);
    for word in ["lemongrass", "paintball", "teahouse"] {
        let idx = lex.index_of(word).unwrap();
        assert!(!lex.is_ranked(idx), "'{word}' should be guess-only");
        let rank = ranking.rank_of(idx);
        assert!((2..=lex.ranked_count() + 1).contains(&rank));
        // Less close than the ranked word before its rank, at least as close as the one at it.
        let sim = lex.similarity(secret, idx);
        assert!(sim < lex.similarity(secret, ranking.word_at(rank - 1)));
        if rank <= lex.ranked_count() {
            assert!(sim >= lex.similarity(secret, ranking.word_at(rank)));
        }
    }
    // Hints and the closest-words list only ever show ranked words.
    let game = ConceptsGame::new(Difficulty::Medium, Some("ocean")).unwrap();
    for w in game.nearest(500) {
        assert!(
            lex.is_ranked(lex.index_of(w).unwrap()),
            "'{w}' isn't ranked"
        );
    }
    // A participle of a ranked word scores as that word.
    assert_eq!(lex.word(lex.resolve("stained").unwrap()), "stain");
    assert!(lex.is_ranked(lex.index_of("camshaft").unwrap()));
}

#[test]
fn test_guessing_inflections_duplicates_and_win() {
    let mut game = ConceptsGame::new(Difficulty::Medium, Some("dog")).unwrap();
    assert_eq!(game.status(), Status::Playing);

    let first = game.make_guess("  Cats ").unwrap();
    assert!(!first.duplicate);
    assert_eq!(first.guess.word, "cat");
    assert_eq!(first.guess.typed.as_deref(), Some("cats"));
    assert!(first.guess.rank > 1);

    // The base form repeats the inflection's guess rather than adding a new one.
    let again = game.make_guess("cat").unwrap();
    assert!(again.duplicate);
    assert_eq!(again.guess.number, 1);
    assert_eq!(game.guesses().len(), 1);

    assert_eq!(
        game.make_guess("the"),
        Err(GuessError::UnknownWord("the".into()))
    );
    assert_eq!(game.make_guess("dog2"), Err(GuessError::NonAlphabetic));
    assert_eq!(game.make_guess("   "), Err(GuessError::Empty));
    assert_eq!(game.guesses().len(), 1);

    game.make_guess("violin").unwrap();
    let ranks: Vec<usize> = game.guesses_by_rank().iter().map(|g| g.rank).collect();
    assert!(ranks.windows(2).all(|p| p[0] <= p[1]));

    let win = game.make_guess("dogs").unwrap();
    assert_eq!(win.guess.rank, 1);
    assert_eq!(game.status(), Status::Won);
    assert_eq!(game.guess_count(), 3);
    assert_eq!(game.make_guess("cat"), Err(GuessError::GameOver));
    assert!(!game.nearest(5).contains(&"dog"));
    assert_eq!(game.nearest(5).len(), 5);
}

#[test]
fn test_hints_halve_best_guess_not_earlier_hints() {
    let lex = lexicon();
    let mut game = ConceptsGame::new(Difficulty::Medium, Some("dog")).unwrap();
    let ranking = lex.rank_against(lex.index_of("dog").unwrap());

    // Before any guess: the word at FIRST_HINT_RANK, then nothing until a guess.
    let first = game.hint().unwrap();
    assert!(first.hint);
    assert_eq!(first.rank, FIRST_HINT_RANK);
    assert!(matches!(game.hint(), Err(HintError::GuessFirst { .. })));
    assert_eq!(game.best_guess_rank(), None);

    // A guess's hint is at half its rank, and only once.
    let animal = game.make_guess("animal").unwrap().guess.rank;
    let h = game.hint().unwrap();
    assert_eq!(h.rank, (animal / 2).max(2));
    assert_eq!(h.word, lex.word(ranking.word_at(h.rank)));
    assert_eq!(game.hint(), Err(HintError::GuessFirst { rank: animal }));

    // A worse guess doesn't move it; a closer one does.
    game.make_guess("violin").unwrap();
    assert!(matches!(game.hint(), Err(HintError::GuessFirst { .. })));
    let close_word = lex.word(ranking.word_at(5));
    game.make_guess(close_word).unwrap();
    assert_eq!(game.hint().unwrap().rank, 2);

    // A best guess of #2 leaves nothing between it and the answer.
    let second = lex.word(ranking.word_at(2));
    let mut game = ConceptsGame::new(Difficulty::Medium, Some("dog")).unwrap();
    game.make_guess(second).unwrap();
    assert_eq!(game.hint(), Err(HintError::NoneCloser));

    // Hints never repeat a word.
    let words: HashSet<&str> = game.guesses().iter().map(|g| g.word.as_str()).collect();
    assert_eq!(words.len(), game.guesses().len());
}

#[test]
fn test_give_up_and_reset() {
    let mut game = ConceptsGame::new(Difficulty::Medium, Some("ocean")).unwrap();
    game.make_guess("sea").unwrap();
    game.give_up();
    assert_eq!(game.status(), Status::GivenUp);
    assert_eq!(game.secret_word(), "ocean");
    assert_eq!(game.hint(), Err(HintError::GameOver));

    game.reset(None);
    assert_eq!(game.status(), Status::Playing);
    assert!(game.guesses().is_empty());
    assert!(game.daily_date().is_none());
    let tier = lexicon().secret_tier(lexicon().index_of(game.secret_word()).unwrap());
    assert!(tier.is_some_and(|t| t <= Difficulty::Medium.max_tier()));

    assert!(ConceptsGame::new(Difficulty::Medium, Some("notaword")).is_err());
}

#[test]
fn test_daily_is_deterministic_and_cycles_without_repeats() {
    let lex = lexicon();
    let tier = DAILY_DIFFICULTY.max_tier();
    let n = DAILY_DIFFICULTY.secret_count() as u64;
    assert_eq!(
        lex.daily_secret(DAILY_EPOCH_DAY + 5, tier),
        lex.daily_secret(DAILY_EPOCH_DAY + 5, tier)
    );
    let cycle: HashSet<usize> = (0..n)
        .map(|d| lex.daily_secret(DAILY_EPOCH_DAY + d, tier))
        .collect();
    assert_eq!(
        cycle.len() as u64,
        n,
        "a daily word repeated within one cycle"
    );
    assert_eq!(
        lex.daily_secret(DAILY_EPOCH_DAY, tier),
        lex.daily_secret(DAILY_EPOCH_DAY + n, tier)
    );

    // Daily is always at DAILY_DIFFICULTY, whatever level was being played.
    let mut game = ConceptsGame::new(Difficulty::Hard, None).unwrap();
    game.start_daily();
    assert_eq!(game.difficulty(), DAILY_DIFFICULTY);
    let daily = ConceptsGame::new_daily();
    assert!(daily.daily_date().is_some());
    assert_eq!(daily.secret_word(), game.secret_word());
    let idx = lex.index_of(daily.secret_word()).unwrap();
    assert!(lex.secret_tier(idx).is_some_and(|t| t <= tier));
}

#[test]
fn test_difficulty_pools_nest_and_sort_concrete_first() {
    let lex = lexicon();
    let (easy, medium, hard) = (lex.secret_pool(1), lex.secret_pool(2), lex.secret_pool(3));
    assert!(easy.len() >= 800, "only {} easy secrets", easy.len());
    assert!(easy.len() < medium.len() && medium.len() < hard.len());
    let medium_set: HashSet<usize> = medium.iter().copied().collect();
    let hard_set: HashSet<usize> = hard.iter().copied().collect();
    assert!(easy.iter().all(|i| medium_set.contains(i)));
    assert!(medium.iter().all(|i| hard_set.contains(i)));

    let tier = |w: &str| lex.secret_tier(lex.index_of(w).unwrap());
    for w in ["elephant", "bread", "guitar", "ocean", "doctor"] {
        assert_eq!(tier(w), Some(1), "'{w}' should be easy");
    }
    for w in ["baseball", "ceremony", "parliament", "birthday"] {
        assert_eq!(tier(w), Some(2), "'{w}' should be medium");
    }
    for w in ["credibility", "democracy", "anxiety", "poverty"] {
        assert_eq!(tier(w), Some(3), "'{w}' should be hard");
    }

    // Each level's random secrets come from its own pool; changing level restarts.
    let mut game = ConceptsGame::new(Difficulty::Easy, None).unwrap();
    for _ in 0..50 {
        assert_eq!(tier(game.secret_word()), Some(1));
        game.reset(None);
    }
    game.make_guess("thing").ok();
    game.reset(Some(Difficulty::Hard));
    assert_eq!(game.difficulty(), Difficulty::Hard);
    assert!(game.guesses().is_empty());

    assert_eq!("e".parse::<Difficulty>(), Ok(Difficulty::Easy));
    assert_eq!(" HARD ".parse::<Difficulty>(), Ok(Difficulty::Hard));
    assert!("insane".parse::<Difficulty>().is_err());
    assert_eq!(Difficulty::Hard.next(), Difficulty::Easy);
}

#[test]
fn test_names_places_and_brands_are_left_out() {
    let lex = lexicon();
    for name in [
        "amazon", "google", "intel", "japan", "paris", "colin", "john", "googled",
    ] {
        assert!(lex.resolve(name).is_none(), "'{name}' should be left out");
    }
    // Words with a common meaning of their own stay, even if they're also names,
    // and so do words that come from names or are only spelled capitalized.
    for word in [
        "robin",
        "creek",
        "university",
        "apple",
        "wed",
        "zipper",
        "turkey",
        "anglomania",
        "anglophobia",
        "americanization",
        "argonaut",
        "birchwood",
        "broadway",
        "stetson",
        "triassic",
    ] {
        assert!(lex.resolve(word).is_some(), "'{word}' should be guessable");
    }
}
