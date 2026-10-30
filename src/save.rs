//! Saved games, so a player can quit and pick up where they left off.
//!
//! Each game keeps one JSON file in `~/.ndt/` (`jotto.json`, `wordle.json`,
//! `contexto.json`). It's rewritten after every change, and holds only what can't be
//! rebuilt: the secret and the guesses (replayed on load, so derived state such as
//! letter colours or ranks is never stale). A finished game isn't kept, so the next
//! launch starts fresh. A `README.txt` beside them says what made the folder.
//!
//! The secret is stored base64-encoded: not hidden, just not readable at a glance.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Bumped when a saved game's shape changes; a save with another version is dropped
/// (with a notice) rather than half-read.
pub const SAVE_VERSION: u32 = 1;

/// Overrides the save folder, for tests (so they never touch a player's saves).
const DIR_ENV: &str = "NDT_SAVE_DIR";

/// The folder saves live in: `$NDT_SAVE_DIR` if set, otherwise `~/.ndt`.
pub fn save_dir() -> Option<PathBuf> {
    match std::env::var_os(DIR_ENV) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => std::env::home_dir().map(|home| home.join(".ndt")),
    }
}

/// A game's save file path (`name` is the command, e.g. "wordle").
pub fn save_path(name: &str) -> Option<PathBuf> {
    save_dir().map(|dir| dir.join(format!("{name}.json")))
}

/// What reading a save file found.
#[derive(Debug)]
pub enum Loaded<T> {
    /// No save yet.
    Missing,
    Found(T),
    /// A save that couldn't be used, and why; the game starts fresh and overwrites it.
    Unusable(String),
}

/// The version stamp every save carries, read first so a save of another version is
/// recognised as such rather than reported as corrupt.
#[derive(serde::Deserialize)]
struct VersionOnly {
    version: u32,
}

/// Reads a save. `T` must carry a top-level `version` field.
pub fn load<T: DeserializeOwned>(path: &Path) -> Loaded<T> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Loaded::Missing,
        Err(e) => return Loaded::Unusable(format!("couldn't read it ({e})")),
    };
    match serde_json::from_str::<VersionOnly>(&text) {
        Ok(v) if v.version != SAVE_VERSION => {
            return Loaded::Unusable("it's from a different version".into())
        }
        Ok(_) => {}
        Err(_) => return Loaded::Unusable("it's damaged".into()),
    }
    match serde_json::from_str(&text) {
        Ok(saved) => Loaded::Found(saved),
        Err(_) => Loaded::Unusable("it's damaged".into()),
    }
}

/// Standard base64 (with `=` padding), so `base64 --decode` reads it.
const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `text` as base64; [`decode`] reverses it.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len().div_ceil(3) * 4);
    for chunk in text.as_bytes().chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes [`encode`]'s base64 (`None` if it isn't valid base64 of UTF-8 text).
pub fn decode(encoded: &str) -> Option<String> {
    let encoded = encoded.trim_end_matches('=');
    let mut bytes = Vec::with_capacity(encoded.len() * 3 / 4);
    let (mut n, mut bits) = (0u32, 0);
    for c in encoded.bytes() {
        n = n << 6 | BASE64.iter().position(|&b| b == c)? as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((n >> bits) as u8);
        }
    }
    String::from_utf8(bytes).ok()
}

/// Serde helpers storing a word base64-encoded: `#[serde(with = "save::encoded")]`.
pub mod encoded {
    use serde::{de, Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(word: &str, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::encode(word))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        let text = String::deserialize(d)?;
        super::decode(&text).ok_or_else(|| de::Error::custom("not base64"))
    }
}

/// `README.txt` for the save folder, so someone who finds a hidden `.ndt` in their
/// home folder can tell what made it and whether it's safe to delete.
pub const FOLDER_README: &str = concat!(
    "This folder was made by jotto-cli, the terminal word games jotto, wordle and\n",
    "contexto (",
    env!("CARGO_PKG_REPOSITORY"),
    ").\n",
    "\n",
    "It holds their saved games, so you can quit a game and pick it up later:\n",
    "\n",
    "  jotto.json, wordle.json, contexto.json\n",
    "      One file per game, holding the game in progress, if any.\n",
    "\n",
    "Deleting a file starts that game fresh, and deleting this folder is safe too:\n",
    "it's made again the next time you play.\n",
    "\n",
    "So a glance doesn't spoil a game, the secret word (\"secret\") is\n",
    "base64-encoded. To read it, or to change it (encode the new word and paste it\n",
    "in its place):\n",
    "\n",
    "  macOS / Linux:\n",
    "      echo 'Y3JhbmU=' | base64 --decode        (prints crane)\n",
    "      printf 'crane' | base64                  (prints Y3JhbmU=)\n",
    "\n",
    "  Windows (PowerShell):\n",
    "      [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('Y3JhbmU='))\n",
    "      [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes('crane'))\n",
    "\n",
    "Use printf, not echo, to encode: echo adds a newline that would be encoded too.\n",
);

/// Writes [`FOLDER_README`] into `dir` unless it's already there as is.
fn write_folder_readme(dir: &Path) -> io::Result<()> {
    let path = dir.join("README.txt");
    if fs::read_to_string(&path).is_ok_and(|text| text == FOLDER_README) {
        return Ok(());
    }
    fs::write(path, FOLDER_README)
}

/// Writes `json` to `path` through a temporary file and a rename, so a crash or a
/// full disk mid-write leaves the old save rather than half a new one.
fn write_atomically(path: &Path, json: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

/// What the player is told about their save when a game opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    /// A saved game was picked up again.
    Resumed(String),
    /// The save couldn't be used, so a new game started.
    Problem(String),
}

impl Notice {
    pub fn text(&self) -> &str {
        match self {
            Self::Resumed(text) | Self::Problem(text) => text,
        }
    }
}

/// A game that can be saved; [`start`] needs its daily date to word the notice.
pub trait Saveable {
    fn daily_date(&self) -> Option<&str>;
}

/// Opens a game from the player's save, the one policy every game follows: the
/// saved game is resumed unless the command line `asked_for_game` (a difficulty, the
/// daily or a word); otherwise `new_game` starts one. `resume` returns `None` when
/// the save has no game in progress, or why it can't be resumed.
pub fn start<S, G: Saveable, E>(
    loaded: Loaded<S>,
    asked_for_game: bool,
    resume: impl FnOnce(&S) -> Result<Option<G>, String>,
    new_game: impl FnOnce() -> Result<G, E>,
) -> Result<(G, Option<Notice>), E> {
    let (saved, mut notice) = match loaded {
        Loaded::Missing => (None, None),
        Loaded::Found(saved) => (Some(saved), None),
        Loaded::Unusable(why) => (
            None,
            Some(Notice::Problem(format!(
                "Your saved game couldn't be loaded: {why}."
            ))),
        ),
    };
    if let Some(saved) = saved.as_ref().filter(|_| !asked_for_game) {
        match resume(saved) {
            Ok(Some(game)) => {
                let text = match game.daily_date() {
                    Some(date) => format!("Resumed your daily game ({date} UTC)."),
                    None => "Resumed your saved game.".to_string(),
                };
                return Ok((game, Some(Notice::Resumed(text))));
            }
            Ok(None) => {}
            Err(why) => {
                notice = Some(Notice::Problem(format!(
                    "Your saved game couldn't be resumed: {why}."
                )))
            }
        }
    }
    Ok((new_game()?, notice))
}

/// Keeps one game's save file in step with the game: [`Autosave::sync`] after each
/// change writes the file only if what it would save is different.
#[derive(Debug)]
pub struct Autosave {
    path: Option<PathBuf>,
    last: String,
    error: Option<String>,
    /// Whether the folder's README has been written this session.
    readme_written: bool,
    /// Shown when the game opens: that a save was resumed, or couldn't be.
    pub notice: Option<Notice>,
}

impl Autosave {
    /// Saves to `path` (none: the home folder couldn't be found, so nothing is saved).
    pub fn new(path: Option<PathBuf>) -> Self {
        let error = path
            .is_none()
            .then(|| "couldn't find your home folder".to_string());
        Self {
            path,
            last: String::new(),
            error,
            readme_written: false,
            notice: None,
        }
    }

    /// Writes `saved` if it changed since the last write. A failure is kept for
    /// [`Autosave::error`] rather than interrupting the game; later changes retry.
    pub fn sync<T: Serialize>(&mut self, saved: &T) {
        let Some(path) = &self.path else { return };
        let json = serde_json::to_string_pretty(saved).expect("saved games serialize");
        if json == self.last {
            return;
        }
        match write_atomically(path, &json) {
            Ok(()) => {
                self.last = json;
                self.error = None;
                // Once a session, after the first save made sure the folder exists.
                // A failure here only loses the explanation, so it isn't reported.
                if !self.readme_written {
                    if let Some(dir) = path.parent() {
                        let _ = write_folder_readme(dir);
                    }
                    self.readme_written = true;
                }
            }
            Err(e) => self.error = Some(format!("couldn't write {} ({e})", path.display())),
        }
    }

    /// Why the latest save failed, if it did.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

/// A `clap` value enum's command-line name ("scrabble"), the form saves store it in,
/// so a save reads the same as the `--difficulty` that would make it.
pub fn enum_name<E: ValueEnum>(value: E) -> String {
    value
        .to_possible_value()
        .expect("difficulties have names")
        .get_name()
        .to_string()
}

/// Parses an [`enum_name`] back.
pub fn parse_enum<E: ValueEnum>(name: &str) -> Option<E> {
    E::from_str(name, true).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Saved {
        version: u32,
        words: Vec<String>,
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ndt-save-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn writes_only_changes_and_reads_back() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("game.json");
        assert!(matches!(load::<Saved>(&path), Loaded::Missing));

        let mut autosave = Autosave::new(Some(path.clone()));
        let saved = Saved {
            version: SAVE_VERSION,
            words: vec!["crane".into()],
        };
        autosave.sync(&saved);
        assert!(autosave.error().is_none());
        assert!(!path.with_extension("json.tmp").exists());
        match load::<Saved>(&path) {
            Loaded::Found(s) => assert_eq!(s, saved),
            other => panic!("expected the save back, got {other:?}"),
        }

        // The folder explains itself.
        assert_eq!(
            fs::read_to_string(dir.join("README.txt")).unwrap(),
            FOLDER_README
        );
        assert!(FOLDER_README.contains("https://github.com/guslipkin/jotto-cli"));

        // An unchanged game isn't rewritten.
        fs::remove_file(&path).unwrap();
        autosave.sync(&saved);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_versions_and_damage_are_unusable() {
        let dir = temp_dir("bad");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("game.json");

        fs::write(&path, r#"{"version": 999, "words": []}"#).unwrap();
        assert!(matches!(load::<Saved>(&path), Loaded::Unusable(why) if why.contains("version")));
        fs::write(&path, "{not json").unwrap();
        assert!(matches!(load::<Saved>(&path), Loaded::Unusable(why) if why.contains("damaged")));
        fs::write(&path, r#"{"version": 1, "words": 3}"#).unwrap();
        assert!(matches!(load::<Saved>(&path), Loaded::Unusable(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn base64_matches_the_standard_tool() {
        // `printf crane | base64` and friends, padding included.
        for (word, b64) in [
            ("crane", "Y3JhbmU="),
            ("ocean", "b2NlYW4="),
            ("dog", "ZG9n"),
            ("paintball", "cGFpbnRiYWxs"),
            ("", ""),
        ] {
            assert_eq!(encode(word), b64);
            assert_eq!(decode(b64).as_deref(), Some(word));
        }
        assert_eq!(decode("not base64!"), None);
    }

    #[test]
    fn enum_names_round_trip() {
        use crate::game::Difficulty;
        assert_eq!(enum_name(Difficulty::Scrabble), "scrabble");
        assert_eq!(
            parse_enum::<Difficulty>("scrabble"),
            Some(Difficulty::Scrabble)
        );
        assert_eq!(parse_enum::<Difficulty>("nope"), None);
    }
}
