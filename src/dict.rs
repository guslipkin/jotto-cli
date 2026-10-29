use std::cmp::Ordering;
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

/// Embedded 7-byte records: 5-byte ASCII lowercase word + 1-byte packed metadata + 1 flags byte.
/// Byte 5, bits 0..=6 (`& 0x7F`): SCOWL difficulty (`1..=95`).
/// Byte 5, bit 7 (`& 0x80`): `1` if all 5 letters are unique (Jotto-eligible), `0` if letters repeat.
/// Byte 6, bit 0 (`& 0x01`): `1` if the word is a plural or -s verb form, which Wordle
/// accepts as a guess but, like the original, never uses as its answer.
/// Sorted lexicographically by the 5-byte word.
static WORDS_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/words.bin"));

const RECORD_SIZE: usize = 7;
const DIFF_MASK: u8 = 0x7F;
const UNIQUE_FLAG: u8 = 0x80;
const PLURAL_FLAG: u8 = 0x01;

/// Fixed UTC epoch day (2024-01-01 UTC = 19,723 days since 1970-01-01) for daily cooldown tracking.
pub const DAILY_EPOCH_DAY: u64 = 19_723;

/// Daily mode always uses the Scrabble (80) SCOWL word list.
pub const DAILY_DIFFICULTY_SIZE: u8 = 80;

const NUM_BUCKETS: usize = 64;

/// Game mode selector shared across dictionary, game engine, CLI, TUI, and plain REPL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameMode {
    #[default]
    Jotto,
    Wordle,
}

impl GameMode {
    /// Lowercase binary/command name (`"jotto"` or `"wordle"`).
    pub const fn command_name(self) -> &'static str {
        match self {
            Self::Jotto => "jotto",
            Self::Wordle => "wordle",
        }
    }

    /// Uppercase display title (`"JOTTO"` or `"WORDLE"`).
    pub const fn title(self) -> &'static str {
        match self {
            Self::Jotto => "JOTTO",
            Self::Wordle => "WORDLE",
        }
    }

    /// Whether words must have 5 unique letters with no repeats.
    pub const fn requires_unique_letters(self) -> bool {
        matches!(self, Self::Jotto)
    }

    /// Maximum guesses allowed per game (`None` for Jotto, `Some(6)` for Wordle).
    pub const fn max_guesses(self) -> Option<usize> {
        match self {
            Self::Jotto => None,
            Self::Wordle => Some(6),
        }
    }
}

#[inline(always)]
fn chunk_matches_mode(chunk: &[u8; RECORD_SIZE], mode: GameMode) -> bool {
    !mode.requires_unique_letters() || (chunk[5] & UNIQUE_FLAG) != 0
}

/// Whether a record can be `mode`'s secret word: any guessable word in Jotto, and
/// anything but a plural or -s form in Wordle.
#[inline(always)]
fn chunk_is_answer(chunk: &[u8; RECORD_SIZE], mode: GameMode) -> bool {
    chunk_matches_mode(chunk, mode) && !(mode == GameMode::Wordle && chunk[6] & PLURAL_FLAG != 0)
}

#[inline(always)]
fn chunk_difficulty(chunk: &[u8; RECORD_SIZE]) -> u8 {
    chunk[5] & DIFF_MASK
}

/// Returns the total number of words in the embedded dictionary for `mode`.
pub fn total_words(mode: GameMode) -> usize {
    WORDS_DATA
        .as_chunks::<RECORD_SIZE>()
        .0
        .iter()
        .filter(|chunk| chunk_matches_mode(chunk, mode))
        .count()
}

/// Checks whether a 5-letter lowercase word exists in the dictionary for `mode` via O(log N) binary search.
pub fn contains(mode: GameMode, word: &str) -> bool {
    word_difficulty(mode, word).is_some()
}

/// Returns the SCOWL difficulty level of a word if it exists in the dictionary for `mode`.
pub fn word_difficulty(mode: GameMode, word: &str) -> Option<u8> {
    let target: &[u8; 5] = word.as_bytes().try_into().ok()?;
    let chunks = WORDS_DATA.as_chunks::<RECORD_SIZE>().0;
    let mut low = 0usize;
    let mut high = chunks.len();

    while low < high {
        let mid = low + (high - low) / 2;
        let chunk = &chunks[mid];
        let entry = &chunk[..5];
        match entry.cmp(target) {
            Ordering::Less => low = mid + 1,
            Ordering::Greater => high = mid,
            Ordering::Equal => {
                if chunk_matches_mode(chunk, mode) {
                    return Some(chunk_difficulty(chunk));
                } else {
                    return None;
                }
            }
        }
    }
    None
}

/// Returns the number of possible secret words for `mode` with `difficulty <= max_difficulty`
/// (in Wordle, plurals and -s forms are guessable but never the answer, so aren't counted).
pub fn word_count(mode: GameMode, max_difficulty: u8) -> usize {
    WORDS_DATA
        .as_chunks::<RECORD_SIZE>()
        .0
        .iter()
        .filter(|chunk| chunk_is_answer(chunk, mode) && chunk_difficulty(chunk) <= max_difficulty)
        .count()
}

/// Picks a random secret word for `mode` with `difficulty <= max_difficulty`,
/// avoiding any word in `excluded` until the entire pool for `(mode, max_difficulty)` has been exhausted.
/// Returns `(word, recycled)` where `recycled` is `true` if `excluded` had exhausted the pool.
pub fn random_word_excluding(
    mode: GameMode,
    max_difficulty: u8,
    excluded: &HashSet<String>,
) -> (&'static str, bool) {
    let mut eligible = WORDS_DATA
        .as_chunks::<RECORD_SIZE>()
        .0
        .iter()
        .filter(|chunk| chunk_is_answer(chunk, mode) && chunk_difficulty(chunk) <= max_difficulty);

    let unplayed_count = eligible
        .clone()
        .filter(|chunk| {
            let w = std::str::from_utf8(&chunk[..5]).expect("valid ASCII");
            !excluded.contains(w)
        })
        .count();

    if unplayed_count > 0 {
        let target_idx = fastrand::usize(0..unplayed_count);
        let chunk = eligible
            .filter(|chunk| {
                let w = std::str::from_utf8(&chunk[..5]).expect("valid ASCII");
                !excluded.contains(w)
            })
            .nth(target_idx)
            .expect("Target unplayed word index out of bounds");
        let word = std::str::from_utf8(&chunk[..5]).expect("Dictionary contains valid ASCII");
        (word, false)
    } else {
        let total = word_count(mode, max_difficulty);
        assert!(
            total > 0,
            "No words found in dictionary for {mode:?} difficulty <= {max_difficulty}"
        );
        let target_idx = fastrand::usize(0..total);
        let chunk = eligible
            .nth(target_idx)
            .expect("Target word index out of bounds");
        let word = std::str::from_utf8(&chunk[..5]).expect("Dictionary contains valid ASCII");
        (word, true)
    }
}

/// Returns the current UTC day number (days since 1970-01-01 00:00:00 UTC).
pub fn current_utc_days() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400
}

/// Converts days since 1970-01-01 UTC into `(year, month, day)` using Howard Hinnant's civil calendar algorithm.
pub const fn utc_ymd_from_days(days_since_epoch: u64) -> (i32, u32, u32) {
    let z = days_since_epoch as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

/// Formats a UTC day number (days since 1970-01-01) as `"YYYY-MM-DD"`.
pub fn format_utc_date(days_since_epoch: u64) -> String {
    let (y, m, d) = utc_ymd_from_days(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Returns the daily cooldown length (in UTC days) for `mode` — 90% of the Scrabble (`80`) pool
/// (well over a decade for each game; the exact length follows the current SCOWL list).
pub fn daily_cooldown_days(mode: GameMode) -> u64 {
    let pool = word_count(mode, DAILY_DIFFICULTY_SIZE);
    ((pool * 9) / 10).max(1) as u64
}

/// Picks today's deterministic UTC daily word (from the Scrabble `80` list for `mode`) and formatted UTC date string (`"YYYY-MM-DD"`).
pub fn daily_word(mode: GameMode) -> (&'static str, String) {
    let utc_day = current_utc_days();
    let word = daily_word_for_day(mode, utc_day);
    let date_str = format_utc_date(utc_day);
    (word, date_str)
}

/// Deterministically selects the daily word for `utc_day` (days since 1970-01-01 UTC) from the
/// Scrabble (`80`) word list for `mode` using Bucketed Rendezvous Hashing (Highest Random Weight)
/// anchored at [`DAILY_EPOCH_DAY`] with a 90%-pool cooldown window ([`daily_cooldown_days`]).
pub fn daily_word_for_day(mode: GameMode, utc_day: u64) -> &'static str {
    let mut buckets: [Vec<(&'static [u8; 5], u64, u64)>; NUM_BUCKETS] =
        std::array::from_fn(|_| Vec::new());
    let mut total_candidates = 0usize;

    for chunk in WORDS_DATA.as_chunks::<RECORD_SIZE>().0 {
        if chunk_is_answer(chunk, mode) && chunk_difficulty(chunk) <= DAILY_DIFFICULTY_SIZE {
            let bytes: &[u8; 5] = chunk[..5].try_into().expect("5-byte word");
            let key = pack_word_u64(bytes);
            let bucket_idx = (hash_word_bucket(key) as usize) % NUM_BUCKETS;
            buckets[bucket_idx].push((bytes, key, 0));
            total_candidates += 1;
        }
    }

    assert!(
        total_candidates > 0,
        "No words found in dictionary for {mode:?} daily difficulty <= {DAILY_DIFFICULTY_SIZE}"
    );

    let cooldown = ((total_candidates * 9) / 10).max(1) as u64;
    let start_day = DAILY_EPOCH_DAY.min(utc_day);
    let mode_salt: u64 = match mode {
        GameMode::Jotto => 0,
        GameMode::Wordle => 0x574F_5244_4C45_5F31, // "WORDLE_1"
    };
    let mut chosen_bytes: &'static [u8; 5] = b"about";

    for day in start_day..=utc_day {
        let primary_b = (hash_day_word(day, mode_salt, 0) as usize) % NUM_BUCKETS;
        let mut best_loc: Option<(usize, usize)> = None;
        let mut best_score = 0u64;

        // Probe starting at primary_b and stop at the first bucket with eligible words
        for step in 0..NUM_BUCKETS {
            let b_idx = (primary_b + step) % NUM_BUCKETS;
            for (i, &(_, word_key, available_at)) in buckets[b_idx].iter().enumerate() {
                if available_at > day {
                    continue;
                }
                let score = hash_day_word(day, mode_salt, word_key);
                if best_loc.is_none() || score > best_score {
                    best_score = score;
                    best_loc = Some((b_idx, i));
                }
            }
            if best_loc.is_some() {
                break;
            }
        }

        let (b_idx, item_idx) = best_loc.expect("At least one word must be eligible");
        buckets[b_idx][item_idx].2 = day + cooldown + 1;
        chosen_bytes = buckets[b_idx][item_idx].0;
    }

    std::str::from_utf8(chosen_bytes).expect("Dictionary contains valid ASCII")
}

#[inline(always)]
fn pack_word_u64(word: &[u8; 5]) -> u64 {
    (word[0] as u64)
        | ((word[1] as u64) << 8)
        | ((word[2] as u64) << 16)
        | ((word[3] as u64) << 24)
        | ((word[4] as u64) << 32)
}

#[inline(always)]
fn hash_word_bucket(word_key: u64) -> u64 {
    let mut z = word_key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Deterministic 64-bit SplitMix64 mixer over `(day, mode_salt, word_key)`.
#[inline(always)]
fn hash_day_word(day: u64, mode_salt: u64, word_key: u64) -> u64 {
    let mut z = day
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((DAILY_DIFFICULTY_SIZE as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(mode_salt)
        ^ word_key.wrapping_mul(0x94D0_49BB_1331_11EB);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Checks if a string contains no duplicate characters.
pub fn has_no_duplicate_chars(word: &str) -> bool {
    let mut mask: u32 = 0;
    for b in word.bytes() {
        if !b.is_ascii_lowercase() {
            return false;
        }
        let bit = 1u32 << (b - b'a');
        if (mask & bit) != 0 {
            return false;
        }
        mask |= bit;
    }
    true
}
