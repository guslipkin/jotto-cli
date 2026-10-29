use std::cmp::Ordering;
use std::sync::OnceLock;

use crate::dict::DAILY_EPOCH_DAY;

/// Word lists and int8 meaning vectors written by `scripts/build-concepts-data.py`
/// (see its docstring for the layout). Committed rather than built by `build.rs`,
/// since the GloVe vectors it comes from are an 822 MB download.
static DATA: &[u8] = include_bytes!("../../data/concepts.bin");

const MAGIC: &[u8; 4] = b"CNCP";
const FORMAT_VERSION: u8 = 2;

/// Fixed seed for the daily order, so every copy of the same build agrees on each day's word.
const DAILY_SEED: u64 = 0x434F_4E43_4550_5453; // "CONCEPTS"

/// The embedded word list: every guessable word, the inflections that map onto them,
/// the secret-word pool, and one meaning vector per guessable word.
///
/// Guessable words are either ranked ("core") or guess-only. Only core words fill
/// ranks, hints and the closest-words list; a guess-only word (a rarer word, whose
/// vector is noisier) is ranked by where it would fall among them.
pub struct Lexicon {
    dim: usize,
    words: Vec<&'static str>,
    core: Vec<bool>,
    core_count: usize,
    aliases: Vec<(&'static str, usize)>,
    /// Secret words with their difficulty tier (1 easy, 2 medium, 3 hard).
    secrets: Vec<(usize, u8)>,
    vectors: &'static [u8],
    norms: Vec<f32>,
}

/// Returns the embedded lexicon, parsed on first use.
pub fn lexicon() -> &'static Lexicon {
    static LEXICON: OnceLock<Lexicon> = OnceLock::new();
    LEXICON.get_or_init(|| Lexicon::parse(DATA))
}

fn read_blob(data: &'static [u8], pos: &mut usize) -> Vec<&'static str> {
    let len = u32::from_le_bytes(data[*pos..*pos + 4].try_into().expect("4 bytes")) as usize;
    *pos += 4;
    let text = std::str::from_utf8(&data[*pos..*pos + len]).expect("concepts.bin text is ASCII");
    *pos += len;
    if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n').collect()
    }
}

impl Lexicon {
    fn parse(data: &'static [u8]) -> Self {
        assert_eq!(&data[..4], MAGIC, "concepts.bin has the wrong magic number");
        assert_eq!(
            data[4], FORMAT_VERSION,
            "concepts.bin has an unknown format version"
        );
        let dim = u16::from_le_bytes([data[6], data[7]]) as usize;
        let mut pos = 8;
        let words = read_blob(data, &mut pos);
        let alias_lines = read_blob(data, &mut pos);
        let secret_words = read_blob(data, &mut pos);
        let core: Vec<bool> = data[pos..pos + words.len()]
            .iter()
            .map(|&b| b == 1)
            .collect();
        pos += words.len();
        let core_count = core.iter().filter(|&&c| c).count();
        let vectors = &data[pos..];
        assert_eq!(
            vectors.len(),
            words.len() * dim,
            "concepts.bin vector section doesn't match its word count"
        );

        let index = |w: &str| {
            words
                .binary_search(&w)
                .unwrap_or_else(|_| panic!("concepts.bin names unknown word '{w}'"))
        };
        let aliases = alias_lines
            .iter()
            .map(|line| {
                let (form, lemma) = line.split_once(' ').expect("alias is 'form lemma'");
                (form, index(lemma))
            })
            .collect();
        let secrets = secret_words
            .iter()
            .map(|line| {
                let (w, tier) = line.split_once(' ').expect("secret is 'word tier'");
                (index(w), tier.parse().expect("secret tier is a number"))
            })
            .collect();
        let norms = vectors
            .chunks_exact(dim)
            .map(|v| {
                let sq: i32 = v.iter().map(|&b| (b as i8 as i32).pow(2)).sum();
                (sq as f32).sqrt().max(1.0)
            })
            .collect();

        Self {
            dim,
            words,
            core,
            core_count,
            aliases,
            secrets,
            vectors,
            norms,
        }
    }

    /// Number of ranked words (the rank of the most distant word).
    pub fn ranked_count(&self) -> usize {
        self.core_count
    }

    /// Number of guessable words, ranked and guess-only (inflections not included).
    pub fn guessable_count(&self) -> usize {
        self.words.len()
    }

    /// Whether word `idx` fills a rank of its own (rather than being guess-only).
    pub fn is_ranked(&self, idx: usize) -> bool {
        self.core[idx]
    }

    /// Secret words up to difficulty tier `max_tier` (1 easy, 2 medium, 3 hard),
    /// in a fixed order.
    pub fn secret_pool(&self, max_tier: u8) -> Vec<usize> {
        self.secrets
            .iter()
            .filter(|&&(_, tier)| tier <= max_tier)
            .map(|&(idx, _)| idx)
            .collect()
    }

    /// Returns the guessable word at `idx`.
    pub fn word(&self, idx: usize) -> &'static str {
        self.words[idx]
    }

    /// Returns the index of a guessable word.
    pub fn index_of(&self, word: &str) -> Option<usize> {
        self.words.binary_search(&word).ok()
    }

    /// Resolves what a player typed to a guessable word: the word itself, or the base
    /// form of an inflection ("mice" -> "mouse", "went" -> "go").
    pub fn resolve(&self, word: &str) -> Option<usize> {
        self.index_of(word).or_else(|| {
            self.aliases
                .binary_search_by(|&(form, _)| form.cmp(word))
                .ok()
                .map(|i| self.aliases[i].1)
        })
    }

    /// Difficulty tier of word `idx` if it can be a secret.
    pub fn secret_tier(&self, idx: usize) -> Option<u8> {
        self.secrets
            .iter()
            .find(|&&(i, _)| i == idx)
            .map(|&(_, tier)| tier)
    }

    fn vector(&self, idx: usize) -> &[u8] {
        &self.vectors[idx * self.dim..(idx + 1) * self.dim]
    }

    /// Cosine similarity between two guessable words.
    pub fn similarity(&self, a: usize, b: usize) -> f32 {
        let dot: i32 = self
            .vector(a)
            .iter()
            .zip(self.vector(b))
            .map(|(&x, &y)| (x as i8 as i32) * (y as i8 as i32))
            .sum();
        dot as f32 / (self.norms[a] * self.norms[b])
    }

    /// Ranks every ranked word by closeness to `secret` (the secret itself is rank 1),
    /// and gives each guess-only word the rank of the first ranked word it isn't
    /// closer than, so it shares that rank.
    pub fn rank_against(&self, secret: usize) -> Ranking {
        let sims: Vec<f32> = (0..self.words.len())
            .map(|i| self.similarity(secret, i))
            .collect();
        let mut order: Vec<u32> = (0..self.words.len() as u32)
            .filter(|&i| self.core[i as usize])
            .collect();
        // The secret always comes first, even if rounding gave a near-twin the same score.
        order.sort_by(|&a, &b| {
            let (a, b) = (a as usize, b as usize);
            (b == secret)
                .cmp(&(a == secret))
                .then(sims[b].partial_cmp(&sims[a]).unwrap_or(Ordering::Equal))
                .then(a.cmp(&b))
        });
        let mut rank = vec![0u32; self.words.len()];
        for (pos, &w) in order.iter().enumerate() {
            rank[w as usize] = pos as u32 + 1;
        }
        for (i, r) in rank.iter_mut().enumerate() {
            if !self.core[i] {
                let closer = order.partition_point(|&w| sims[w as usize] > sims[i]);
                // Never #1: that's the secret's alone.
                *r = (closer as u32 + 1).max(2);
            }
        }
        Ranking { order, rank }
    }

    /// The secret for UTC day `utc_day`: the pool up to `max_tier` in a fixed shuffled
    /// order, one a day from [`DAILY_EPOCH_DAY`], so no word repeats until every one
    /// has been used.
    pub fn daily_secret(&self, utc_day: u64, max_tier: u8) -> usize {
        let pool = self.secret_pool(max_tier);
        let n = pool.len();
        let mut order: Vec<usize> = (0..n).collect();
        let mut state = DAILY_SEED;
        for i in (1..n).rev() {
            let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let day = utc_day.saturating_sub(DAILY_EPOCH_DAY) as usize;
        pool[order[day % n]]
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Every guessable word's rank against one secret.
#[derive(Debug, Clone)]
pub struct Ranking {
    order: Vec<u32>,
    rank: Vec<u32>,
}

impl Ranking {
    /// Rank of word `idx` (1 = the secret).
    pub fn rank_of(&self, idx: usize) -> usize {
        self.rank[idx] as usize
    }

    /// Ranked word index at `rank` (1-based).
    pub fn word_at(&self, rank: usize) -> usize {
        self.order[rank - 1] as usize
    }
}
