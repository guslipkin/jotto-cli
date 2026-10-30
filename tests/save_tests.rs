use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use jotto::contexto::game::{self as ctx, ContextoGame};
use jotto::game::{Difficulty, GameMode, GameState, GameStatus, ScratchState};
use jotto::save::{Loaded, Notice};

/// Through JSON and back, as a save file would go.
fn reload<T: serde::Serialize + serde::de::DeserializeOwned>(saved: &T) -> T {
    serde_json::from_str(&serde_json::to_string(saved).unwrap()).unwrap()
}

#[test]
fn test_jotto_game_round_trips_with_scratch_pad() {
    let mut game =
        GameState::new_with_mode(GameMode::Jotto, Difficulty::Easy, Some("crane")).unwrap();
    game.make_guess("plumb").unwrap();
    game.make_guess("north").unwrap();
    game.toggle_scratch('z');
    game.toggle_scratch('q');
    game.toggle_scratch('q');

    let saved = reload(&game.to_saved());
    let back = GameState::from_saved(GameMode::Jotto, &saved)
        .unwrap()
        .unwrap();
    assert_eq!(back.secret_word(), "crane");
    assert_eq!(back.difficulty(), Difficulty::Easy);
    assert_eq!(back.history(), game.history());
    assert_eq!(back.letter_states(), game.letter_states());
    assert_eq!(back.used_letters(), game.used_letters());
    assert_eq!(back.scratched_letters()[25], ScratchState::ScratchedOut);
    assert_eq!(back.scratched_letters()[16], ScratchState::Positive);
    assert_eq!(back.to_saved(), saved);
}

#[test]
fn test_save_file_does_not_show_the_secret() {
    let mut game =
        GameState::new_with_mode(GameMode::Wordle, Difficulty::Easy, Some("crane")).unwrap();
    game.make_guess("plumb").unwrap();
    let json = serde_json::to_string(&game.to_saved()).unwrap();
    assert!(!json.contains("crane"), "{json}");
    assert!(json.contains("Y3JhbmU="), "{json}");
    assert!(json.contains("plumb"), "guesses stay readable: {json}");

    let game = ContextoGame::new(ctx::Difficulty::Medium, Some("ocean")).unwrap();
    let json = serde_json::to_string(&game.to_saved()).unwrap();
    assert!(!json.contains("ocean"), "{json}");
}

#[test]
fn test_wordle_has_no_scratch_pad() {
    let mut game =
        GameState::new_with_mode(GameMode::Wordle, Difficulty::Easy, Some("crane")).unwrap();
    game.toggle_scratch('z');
    assert!(game
        .scratched_letters()
        .iter()
        .all(|&s| s == ScratchState::Default));
    let saved = game.to_saved();
    assert_eq!(saved.game.as_ref().unwrap().scratch, None);
    assert!(!serde_json::to_string(&saved).unwrap().contains("scratch"));

    // A scratch pad in a Wordle save is ignored rather than loaded.
    let mut saved = saved;
    saved.game.as_mut().unwrap().scratch = Some("x".repeat(26));
    let back = GameState::from_saved(GameMode::Wordle, &saved)
        .unwrap()
        .unwrap();
    assert!(back
        .scratched_letters()
        .iter()
        .all(|&s| s == ScratchState::Default));
}

#[test]
fn test_finished_game_is_not_kept() {
    let mut game =
        GameState::new_with_mode(GameMode::Wordle, Difficulty::Easy, Some("crane")).unwrap();
    assert!(game.to_saved().game.is_some());
    game.make_guess("crane").unwrap();
    assert_eq!(game.status(), GameStatus::Won);
    let saved = game.to_saved();
    assert!(saved.game.is_none());
    assert!(GameState::from_saved(GameMode::Wordle, &saved)
        .unwrap()
        .is_none());
}

#[test]
fn test_start_resumes_unless_a_game_is_asked_for() {
    let mut game =
        GameState::new_with_mode(GameMode::Wordle, Difficulty::Scrabble, Some("crane")).unwrap();
    game.make_guess("plumb").unwrap();
    let saved = game.to_saved();

    let (resumed, notice) = GameState::start(
        GameMode::Wordle,
        None,
        false,
        None,
        Loaded::Found(saved.clone()),
    )
    .unwrap();
    assert_eq!(resumed.secret_word(), "crane");
    assert_eq!(resumed.history().len(), 1);
    assert_eq!(
        notice,
        Some(Notice::Resumed("Resumed your saved game.".into()))
    );

    // Asking for a difficulty starts a new game.
    let (fresh, notice) = GameState::start(
        GameMode::Wordle,
        Some(Difficulty::Scrabble),
        false,
        None,
        Loaded::Found(saved.clone()),
    )
    .unwrap();
    assert!(notice.is_none());
    assert!(fresh.history().is_empty());
    assert_eq!(fresh.difficulty(), Difficulty::Scrabble);

    // A custom word starts that game.
    let (custom, _) = GameState::start(
        GameMode::Wordle,
        None,
        false,
        Some("slate"),
        Loaded::Found(saved),
    )
    .unwrap();
    assert_eq!(custom.secret_word(), "slate");
}

#[test]
fn test_start_reports_unusable_saves_and_starts_fresh() {
    let (game, notice) = GameState::start(
        GameMode::Jotto,
        None,
        false,
        None,
        Loaded::Unusable("it's damaged".into()),
    )
    .unwrap();
    assert!(game.history().is_empty());
    assert!(matches!(notice, Some(Notice::Problem(t)) if t.contains("damaged")));

    // A guess that no longer works (here, a repeated letter in Jotto) drops the save.
    let mut saved = GameState::new_with_mode(GameMode::Jotto, Difficulty::Easy, Some("crane"))
        .unwrap()
        .to_saved();
    saved.game.as_mut().unwrap().guesses.push("aaaaa".into());
    let (game, notice) =
        GameState::start(GameMode::Jotto, None, false, None, Loaded::Found(saved)).unwrap();
    assert!(game.history().is_empty());
    assert!(matches!(notice, Some(Notice::Problem(t)) if t.contains("aaaaa")));
}

#[test]
fn test_contexto_game_round_trips_with_hints_and_inflections() {
    let mut game = ContextoGame::new(ctx::Difficulty::Medium, Some("dog")).unwrap();
    game.hint().unwrap();
    game.make_guess("violins").unwrap();
    game.make_guess("animal").unwrap();
    game.hint().unwrap();

    let saved = reload(&game.to_saved());
    let back = ContextoGame::from_saved(&saved).unwrap().unwrap();
    assert_eq!(back.secret_word(), "dog");
    assert_eq!(back.guesses(), game.guesses());
    assert_eq!(back.to_saved(), saved);

    let (resumed, notice) = ContextoGame::start(None, false, None, Loaded::Found(saved)).unwrap();
    assert_eq!(resumed.guesses().len(), 4);
    assert!(matches!(notice, Some(Notice::Resumed(_))));
}

#[test]
fn test_contexto_daily_resume_names_its_date_and_bad_words_drop_the_save() {
    let daily = ContextoGame::new_daily();
    let date = daily.daily_date().unwrap().to_string();
    let (_, notice) =
        ContextoGame::start(None, false, None, Loaded::Found(daily.to_saved())).unwrap();
    assert_eq!(
        notice,
        Some(Notice::Resumed(format!(
            "Resumed your daily game ({date} UTC)."
        )))
    );

    let mut saved = daily.to_saved();
    saved.game.as_mut().unwrap().secret = "notaword".into();
    let (game, notice) = ContextoGame::start(None, false, None, Loaded::Found(saved)).unwrap();
    assert!(game.guesses().is_empty());
    assert!(matches!(notice, Some(Notice::Problem(t)) if t.contains("notaword")));
}

/// A fresh save folder for one test, under the system temp folder.
fn temp_save_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ndt-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Runs a game binary in plain mode with `input`, saving into `dir`.
fn play(bin: &str, dir: &Path, args: &[&str], input: &str) -> String {
    let mut child = Command::new(bin)
        .args(["--plain"])
        .args(args)
        .env("NDT_SAVE_DIR", dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn test_binaries_save_and_resume() {
    let dir = temp_save_dir("binaries");
    let wordle = env!("CARGO_BIN_EXE_wordle");
    play(wordle, &dir, &["--word", "crane"], "plumb\n:quit\n");
    assert!(dir.join("wordle.json").exists());
    let out = play(wordle, &dir, &[], "crane\n");
    assert!(out.contains("Resumed your saved game."), "{out}");
    assert!(out.contains("~P~ ~L~ ~U~ ~M~ ~B~"), "{out}");
    assert!(out.to_lowercase().contains("congratulations"), "{out}");
    // Winning cleared the game, so the next launch starts a new one.
    let out = play(wordle, &dir, &[], ":quit\n");
    assert!(!out.contains("Resumed"), "{out}");

    let contexto = env!("CARGO_BIN_EXE_contexto");
    play(contexto, &dir, &["--word", "ocean"], "sea\n");
    let out = play(contexto, &dir, &[], ":quit\n");
    assert!(out.contains("Resumed your saved game."), "{out}");
    assert!(out.contains("SEA"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}
