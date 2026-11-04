//! Saved games, so a player can quit and pick up where they left off.
//!
//! Every game saves into one JSON file, `~/.ndt/saves.json`, an object with a
//! section per game (`"jotto"`, `"wordle"`, `"contexto"`, `"flights"`). A game's
//! section is rewritten after every change, and holds only what can't be rebuilt:
//! the secret and the guesses (replayed on load, so derived state such as letter
//! colours or ranks is never stale). A finished game isn't kept, so the next launch
//! starts fresh. A `README.txt` beside it says what made the folder.
//!
//! Each section carries its own version and is read on its own, so one game's
//! damaged or outdated save never costs another game its save. Saves from before
//! the shared file (`jotto.json` and so on) are moved into it the first time it's
//! opened.
//!
//! The secret is stored base64-encoded: not hidden, just not readable at a glance.
//!
//! In the Docker image, a session's saves are thrown away when it ends, so they
//! are also kept in the player's browser (see [`mirror_to_browser`]): the
//! terminal carries them, as escape sequences the image's page answers.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use clap::ValueEnum;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

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

/// The file every game saves in, inside [`save_dir`].
pub const SAVES_FILE: &str = "saves.json";

/// The games that once kept a file each (`jotto.json`, ...), moved into
/// [`SAVES_FILE`] when it's first opened.
const LEGACY_GAMES: [&str; 4] = ["jotto", "wordle", "contexto", "flights"];

/// Where one game's save lives: its section of [`SAVES_FILE`] in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    dir: PathBuf,
    name: String,
}

impl Slot {
    /// `name`'s section in `dir`'s saves file.
    pub fn new(dir: impl Into<PathBuf>, name: &str) -> Self {
        Self {
            dir: dir.into(),
            name: name.to_string(),
        }
    }

    /// A game's slot in [`save_dir`] (`name` is the command, e.g. "wordle"), or
    /// `None` when there's no home folder to save in.
    pub fn for_game(name: &str) -> Option<Self> {
        save_dir().map(|dir| Self::new(dir, name))
    }

    /// The saves file this slot is a section of.
    pub fn file(&self) -> PathBuf {
        self.dir.join(SAVES_FILE)
    }
}

/// What reading a save found.
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
#[derive(Deserialize)]
struct VersionOnly {
    version: u32,
}

type Sections = serde_json::Map<String, serde_json::Value>;

/// The saves file's sections (none if it doesn't exist yet), or why it can't be
/// read: `Err(None)` when it's damaged, so writing it afresh is the fix, and
/// `Err(Some(..))` when it couldn't be read at all, so it mustn't be overwritten.
fn read_sections(path: &Path) -> Result<Sections, Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|_| None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Sections::new()),
        Err(e) => Err(Some(format!("couldn't read it ({e})"))),
    }
}

/// Moves the per-game save files from before [`SAVES_FILE`] into `sections`, which
/// is written to the saves file before the old files are deleted, so a failure
/// part way loses nothing. An old file that isn't JSON is unusable anyway (the game
/// would have overwritten it), so it's simply deleted; one whose game already has a
/// section is stale and goes too.
fn migrate_legacy(dir: &Path, sections: &mut Sections) {
    let found: Vec<PathBuf> = LEGACY_GAMES
        .iter()
        .filter_map(|name| {
            let path = dir.join(format!("{name}.json"));
            let text = fs::read_to_string(&path).ok()?;
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                sections.entry(*name).or_insert(value);
            }
            Some(path)
        })
        .collect();
    if found.is_empty() {
        return;
    }
    let json = serde_json::to_string_pretty(sections).expect("saves serialize");
    if write_atomically(&dir.join(SAVES_FILE), &json).is_ok() {
        for path in found {
            let _ = fs::remove_file(path);
        }
    }
}

/// Reads a game's save from its section. `T` must carry a top-level `version` field.
pub fn load<T: DeserializeOwned>(slot: &Slot) -> Loaded<T> {
    let path = slot.file();
    let mut sections = match read_sections(&path) {
        Ok(sections) => sections,
        Err(Some(why)) => return Loaded::Unusable(why),
        Err(None) => return Loaded::Unusable("the saves file is damaged".into()),
    };
    migrate_legacy(&slot.dir, &mut sections);
    let Some(section) = sections.remove(&slot.name) else {
        return Loaded::Missing;
    };
    match VersionOnly::deserialize(&section) {
        Ok(v) if v.version != SAVE_VERSION => {
            return Loaded::Unusable("it's from a different version".into())
        }
        Ok(_) => {}
        Err(_) => return Loaded::Unusable("it's damaged".into()),
    }
    match T::deserialize(section) {
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
    "This folder was made by jotto-cli, the terminal games jotto, wordle, contexto\n",
    "and flights (",
    env!("CARGO_PKG_REPOSITORY"),
    ").\n",
    "\n",
    "It holds their saved games, so you can quit a game and pick it up later:\n",
    "\n",
    "  saves.json\n",
    "      One section per game (\"jotto\", \"wordle\", \"contexto\", \"flights\"),\n",
    "      holding the game in progress, if any, and Flights' settings.\n",
    "\n",
    "Deleting a game's section starts that game fresh, deleting the file starts\n",
    "them all fresh, and deleting this folder is safe too: it's made again the\n",
    "next time you play.\n",
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
/// full disk mid-write leaves the old save rather than half a new one. The
/// temporary file is named for the process, so two games saving at once can't
/// write into each other's.
fn write_atomically(path: &Path, json: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
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

/// The private OSC (operating system command) number saves travel under between
/// a game and the Docker image's page, which keeps them in the browser.
pub const BROWSER_OSC: u32 = 7681;

/// Asks the page for the saves it kept: `ESC ] 7681 ; load BEL`. The page types
/// back `saves:` and the saves file, base64, then Enter (see `docker/`).
pub const BROWSER_LOAD: &str = "\x1b]7681;load\x07";

/// Whether each write of a section is also sent to the page.
static MIRROR: AtomicBool = AtomicBool::new(false);

/// From now on, every section written is also sent to the terminal as
/// `ESC ] 7681 ; save ; <game> ; <base64 section> BEL`, for the page to keep.
/// A section at a time, not the file, so two tabs playing different games
/// don't overwrite each other's. A terminal that doesn't know the sequence
/// ignores it.
pub fn mirror_to_browser() {
    MIRROR.store(true, Ordering::Relaxed);
}

/// Sends a written section to the page. Written whole and flushed at once, so
/// it lands between the frames the game draws, never inside one.
fn send_to_browser(name: &str, section: &str) -> io::Result<()> {
    let mut out = io::stdout().lock();
    write!(
        out,
        "\x1b]{BROWSER_OSC};save;{name};{}\x07",
        encode(section)
    )?;
    out.flush()
}

/// Writes saves the page kept from earlier sessions into `dir`'s saves file,
/// if they're an object of sections, as the file is (whatever's in each is
/// checked by its game when it loads). Says whether it did.
pub fn restore(dir: &Path, json: &str) -> bool {
    let Ok(sections) = serde_json::from_str::<Sections>(json) else {
        return false;
    };
    let text = serde_json::to_string_pretty(&sections).expect("saves serialize");
    write_atomically(&dir.join(SAVES_FILE), &text).is_ok()
}

/// Keeps one game's section of the saves file in step with the game:
/// [`Autosave::sync`] after each change writes it only if what it would save is
/// different.
#[derive(Debug)]
pub struct Autosave {
    slot: Option<Slot>,
    last: String,
    error: Option<String>,
    /// Whether the folder's README has been written this session.
    readme_written: bool,
    /// Shown when the game opens: that a save was resumed, or couldn't be.
    pub notice: Option<Notice>,
}

impl Autosave {
    /// Saves to `slot` (none: the home folder couldn't be found, so nothing is saved).
    pub fn new(slot: Option<Slot>) -> Self {
        let error = slot
            .is_none()
            .then(|| "couldn't find your home folder".to_string());
        Self {
            slot,
            last: String::new(),
            error,
            readme_written: false,
            notice: None,
        }
    }

    /// Writes `saved` if it changed since the last write. A failure is kept for
    /// [`Autosave::error`] rather than interrupting the game; later changes retry.
    ///
    /// The file is read afresh and only this game's section replaced, so a game
    /// open in another terminal keeps its save. A damaged file is written anew
    /// (its sections were unreadable anyway); one that can't be read isn't touched.
    pub fn sync<T: Serialize>(&mut self, saved: &T) {
        let Some(slot) = &self.slot else { return };
        let section = serde_json::to_value(saved).expect("saved games serialize");
        let json = section.to_string();
        if json == self.last {
            return;
        }
        let path = slot.file();
        let mut sections = match read_sections(&path) {
            Ok(sections) => sections,
            Err(None) => Sections::new(),
            Err(Some(why)) => {
                self.error = Some(format!("couldn't update {} ({why})", path.display()));
                return;
            }
        };
        sections.insert(slot.name.clone(), section);
        let text = serde_json::to_string_pretty(&sections).expect("saves serialize");
        match write_atomically(&path, &text) {
            Ok(()) => {
                if MIRROR.load(Ordering::Relaxed) {
                    // Lost only from the browser; the session's file has it.
                    let _ = send_to_browser(&slot.name, &json);
                }
                self.last = json;
                self.error = None;
                // Once a session, after the first save made sure the folder exists.
                // A failure here only loses the explanation, so it isn't reported.
                if !self.readme_written {
                    let _ = write_folder_readme(&slot.dir);
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

/// A game's save holding at most one unfinished game, `P` (Flights' saves more).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGames<P> {
    pub version: u32,
    /// The unfinished game, if any.
    pub game: Option<P>,
}

/// A saved game's difficulty, by its name.
pub fn saved_difficulty<E: ValueEnum>(name: &str) -> Result<E, String> {
    parse_enum(name).ok_or_else(|| format!("unknown difficulty '{name}'"))
}

/// The variant after `value`, in declaration order and wrapping round: how
/// difficulties and units cycle.
pub fn next_variant<E: ValueEnum + PartialEq>(value: &E) -> E {
    let all = E::value_variants();
    let i = all.iter().position(|v| v == value).expect("a variant");
    all[(i + 1) % all.len()].clone()
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
    fn restores_saves_the_browser_kept() {
        let dir = temp_dir("restore");
        let slot = Slot::new(&dir, "wordle");
        // Only an object of sections, as the file is.
        assert!(!restore(&dir, "[1, 2]"));
        assert!(!restore(&dir, "not json"));
        assert!(matches!(load::<Saved>(&slot), Loaded::Missing));
        let json =
            format!(r#"{{"wordle":{{"version":{SAVE_VERSION},"words":["crane"]}},"other":{{}}}}"#);
        assert!(restore(&dir, &json));
        match load::<Saved>(&slot) {
            Loaded::Found(saved) => assert_eq!(saved.words, vec!["crane".to_string()]),
            other => panic!("{other:?}"),
        }
        // A game's next write keeps the other sections.
        let mut autosave = Autosave::new(Some(slot.clone()));
        autosave.sync(&Saved {
            version: SAVE_VERSION,
            words: vec!["slate".into()],
        });
        let text = fs::read_to_string(slot.file()).unwrap();
        assert!(
            text.contains("slate") && text.contains("\"other\""),
            "{text}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_only_changes_and_reads_back() {
        let dir = temp_dir("roundtrip");
        let slot = Slot::new(&dir, "wordle");
        let path = slot.file();
        assert!(matches!(load::<Saved>(&slot), Loaded::Missing));

        let mut autosave = Autosave::new(Some(slot.clone()));
        let saved = Saved {
            version: SAVE_VERSION,
            words: vec!["crane".into()],
        };
        autosave.sync(&saved);
        assert!(autosave.error().is_none());
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            2,
            "only the saves file and README, no temporary file left"
        );
        match load::<Saved>(&slot) {
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
    fn games_share_the_file_without_overwriting_each_other() {
        let dir = temp_dir("shared");
        let (wordle, jotto) = (Slot::new(&dir, "wordle"), Slot::new(&dir, "jotto"));
        let save = |words: &[&str]| Saved {
            version: SAVE_VERSION,
            words: words.iter().map(|w| w.to_string()).collect(),
        };
        // Two games open at once, each saving after the other.
        let mut a = Autosave::new(Some(wordle.clone()));
        let mut b = Autosave::new(Some(jotto.clone()));
        a.sync(&save(&["crane"]));
        b.sync(&save(&["plumb"]));
        a.sync(&save(&["crane", "slate"]));
        assert!(
            matches!(load::<Saved>(&wordle), Loaded::Found(s) if s == save(&["crane", "slate"]))
        );
        assert!(matches!(load::<Saved>(&jotto), Loaded::Found(s) if s == save(&["plumb"])));

        let text = fs::read_to_string(wordle.file()).unwrap();
        let sections: Sections = serde_json::from_str(&text).unwrap();
        assert_eq!(sections.keys().collect::<Vec<_>>(), ["jotto", "wordle"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn other_versions_and_damage_are_unusable() {
        let dir = temp_dir("bad");
        fs::create_dir_all(&dir).unwrap();
        let slot = Slot::new(&dir, "wordle");
        let path = slot.file();
        let good = r#""jotto": {"version": 1, "words": ["plumb"]}"#;

        // One game's bad section leaves the others readable.
        for (section, why) in [
            (r#"{"version": 999, "words": []}"#, "version"),
            (r#"{"version": 1, "words": 3}"#, "damaged"),
            ("[]", "damaged"),
        ] {
            fs::write(&path, format!(r#"{{"wordle": {section}, {good}}}"#)).unwrap();
            assert!(matches!(load::<Saved>(&slot), Loaded::Unusable(w) if w.contains(why)));
            assert!(matches!(
                load::<Saved>(&Slot::new(&dir, "jotto")),
                Loaded::Found(_)
            ));
        }

        // A damaged file is reported, then written afresh by the next save.
        fs::write(&path, "{not json").unwrap();
        assert!(matches!(load::<Saved>(&slot), Loaded::Unusable(why) if why.contains("damaged")));
        let saved = Saved {
            version: SAVE_VERSION,
            words: vec![],
        };
        let mut autosave = Autosave::new(Some(slot.clone()));
        autosave.sync(&saved);
        assert!(autosave.error().is_none());
        assert!(matches!(load::<Saved>(&slot), Loaded::Found(s) if s == saved));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_per_game_files_move_into_the_saves_file() {
        let dir = temp_dir("legacy");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("wordle.json"),
            r#"{"version": 1, "words": ["crane"]}"#,
        )
        .unwrap();
        fs::write(
            dir.join("jotto.json"),
            r#"{"version": 1, "words": ["plumb"]}"#,
        )
        .unwrap();
        fs::write(dir.join("contexto.json"), "{not json").unwrap();
        fs::write(dir.join("notes.json"), "{}").unwrap();

        // Opening any one game moves every old file.
        let jotto = Slot::new(&dir, "jotto");
        assert!(matches!(load::<Saved>(&jotto), Loaded::Found(s) if s.words == ["plumb"]));
        for name in ["wordle", "jotto", "contexto"] {
            assert!(
                !dir.join(format!("{name}.json")).exists(),
                "{name}.json left"
            );
        }
        assert!(
            dir.join("notes.json").exists(),
            "files not ours are left alone"
        );
        let wordle = Slot::new(&dir, "wordle");
        assert!(matches!(load::<Saved>(&wordle), Loaded::Found(s) if s.words == ["crane"]));
        assert!(matches!(
            load::<Saved>(&Slot::new(&dir, "contexto")),
            Loaded::Missing
        ));

        // An old file beside a section that's already there is stale.
        fs::write(
            dir.join("wordle.json"),
            r#"{"version": 1, "words": ["stale"]}"#,
        )
        .unwrap();
        assert!(matches!(load::<Saved>(&wordle), Loaded::Found(s) if s.words == ["crane"]));
        assert!(!dir.join("wordle.json").exists());
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
