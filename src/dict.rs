use std::cmp::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

/// Embedded 6-byte records: 5-byte ASCII lowercase word + 1-byte difficulty.
/// Sorted lexicographically by the 5-byte word.
static WORDS_DATA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/words.bin"));

const RECORD_SIZE: usize = 6;

/// Number of UTC days before a daily word is eligible to be recycled again (~13.7 years).
pub const DAILY_COOLDOWN_DAYS: u64 = 5_000;

/// Fixed UTC epoch day (2024-01-01 UTC = 19,723 days since 1970-01-01) for daily cooldown tracking.
pub const DAILY_EPOCH_DAY: u64 = 19_723;

/// Daily mode always uses the Scrabble (80) SCOWL word list (~5,768 words).
pub const DAILY_DIFFICULTY_SIZE: u8 = 80;

const NUM_BUCKETS: usize = 64;

/// Returns the total number of words in the embedded dictionary.
pub fn total_words() -> usize {
    WORDS_DATA.len() / RECORD_SIZE
}

/// Checks whether a 5-letter lowercase word exists in the dictionary via O(log N) binary search.
pub fn contains(word: &str) -> bool {
    word_difficulty(word).is_some()
}

/// Returns the SCOWL difficulty level of a word if it exists in the dictionary.
pub fn word_difficulty(word: &str) -> Option<u8> {
    let target: &[u8; 5] = word.as_bytes().try_into().ok()?;
    let mut low = 0usize;
    let mut high = total_words();

    while low < high {
        let mid = low + (high - low) / 2;
        let offset = mid * RECORD_SIZE;
        let entry = &WORDS_DATA[offset..offset + 5];
        match entry.cmp(target) {
            Ordering::Less => low = mid + 1,
            Ordering::Greater => high = mid,
            Ordering::Equal => return Some(WORDS_DATA[offset + 5]),
        }
    }
    None
}

/// Returns the number of dictionary words with `difficulty <= max_difficulty`.
pub fn word_count(max_difficulty: u8) -> usize {
    WORDS_DATA
        .as_chunks::<RECORD_SIZE>()
        .0
        .iter()
        .filter(|chunk| chunk[5] <= max_difficulty)
        .count()
}

/// Picks a random 5-letter word from the dictionary with `difficulty <= max_difficulty`.
pub fn random_word(max_difficulty: u8) -> &'static str {
    let count = word_count(max_difficulty);
    assert!(
        count > 0,
        "No words found in dictionary for difficulty <= {max_difficulty}"
    );
    let target_idx = fastrand::usize(0..count);
    let chunk = WORDS_DATA
        .as_chunks::<RECORD_SIZE>()
        .0
        .iter()
        .filter(|chunk| chunk[5] <= max_difficulty)
        .nth(target_idx)
        .expect("Target word index out of bounds");

    std::str::from_utf8(&chunk[..5]).expect("Dictionary contains valid ASCII")
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

/// Picks today's deterministic UTC daily word (from the Scrabble `80` list) and formatted UTC date string (`"YYYY-MM-DD"`).
pub fn daily_word() -> (&'static str, String) {
    let utc_day = current_utc_days();
    let word = daily_word_for_day(utc_day);
    let date_str = format_utc_date(utc_day);
    (word, date_str)
}

/// Deterministically selects the daily word for `utc_day` (days since 1970-01-01 UTC) from the
/// Scrabble (`80`) word list using Bucketed Rendezvous Hashing (Highest Random Weight) anchored at
/// [`DAILY_EPOCH_DAY`] with a [`DAILY_COOLDOWN_DAYS`] (5,000-day / ~13.7-year) cooldown window.
///
/// Why Bucketed Rendezvous Hashing + 5,000-day cooldown?
/// 1. **Resilience to Word List Updates**: Each word's bucket `hash_word(word) % 64` and daily score
///    `hash(day, word)` depend only on the word's 5 letters, never its array index or total
///    word count. If SCOWL adds or removes words in an update, existing words retain their exact buckets
///    and scores, keeping the daily word unchanged >99.9% of the time.
/// 2. **Controlled Recycling (~13.7-Year Cooldown)**: Words chosen in the prior 5,000 days are placed on
///    cooldown so no word can repeat for over 13.5 years, after which words are recycled back into the pool.
pub fn daily_word_for_day(utc_day: u64) -> &'static str {
    let mut buckets: [Vec<(&'static [u8; 5], u64, u64)>; NUM_BUCKETS] =
        std::array::from_fn(|_| Vec::new());
    let mut total_candidates = 0usize;

    for chunk in WORDS_DATA.as_chunks::<RECORD_SIZE>().0 {
        if chunk[5] <= DAILY_DIFFICULTY_SIZE {
            let bytes: &[u8; 5] = chunk[..5].try_into().expect("5-byte word");
            let key = pack_word_u64(bytes);
            let bucket_idx = (hash_word_bucket(key) as usize) % NUM_BUCKETS;
            buckets[bucket_idx].push((bytes, key, 0));
            total_candidates += 1;
        }
    }

    assert!(
        total_candidates > 0,
        "No words found in dictionary for daily difficulty <= {DAILY_DIFFICULTY_SIZE}"
    );

    let cooldown = DAILY_COOLDOWN_DAYS.min((total_candidates.saturating_sub(1)) as u64);
    let start_day = DAILY_EPOCH_DAY.min(utc_day);
    let mut chosen_bytes: &'static [u8; 5] = b"about";

    for day in start_day..=utc_day {
        let primary_b = (hash_day_word(day, 0) as usize) % NUM_BUCKETS;
        let mut best_loc: Option<(usize, usize)> = None;
        let mut best_score = 0u64;

        // Probe starting at primary_b and stop at the first bucket with eligible words
        for step in 0..NUM_BUCKETS {
            let b_idx = (primary_b + step) % NUM_BUCKETS;
            for (i, &(_, word_key, available_at)) in buckets[b_idx].iter().enumerate() {
                if available_at > day {
                    continue;
                }
                let score = hash_day_word(day, word_key);
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

/// Deterministic 64-bit SplitMix64 mixer over `(day, word_key)`.
#[inline(always)]
fn hash_day_word(day: u64, word_key: u64) -> u64 {
    let mut z = day
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((DAILY_DIFFICULTY_SIZE as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
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
