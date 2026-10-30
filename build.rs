use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Bytes per words.bin record: the word, its packed difficulty, and its flags.
const RECORD_SIZE: usize = 7;

const SCOWL_REPO_URL: &str = "https://github.com/en-wl/wordlist.git";
const BUILD_FORMAT_VERSION: &str = "v4-plural-flag-fallback-pos";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=JOTTO_UPDATE_WORDS");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let wordlist_dir = out_dir.join("wordlist");
    let words_bin_path = out_dir.join("words.bin");
    let sha_cache_path = out_dir.join("words.bin.sha");

    // Clone or update the upstream SCOWL v2 repository
    update_or_clone_scowl(&wordlist_dir, &words_bin_path);

    let current_sha = get_git_head_sha(&wordlist_dir)
        .map(|sha| format!("{sha}:{BUILD_FORMAT_VERSION}"))
        .unwrap_or_default();
    let cached_sha = fs::read_to_string(&sha_cache_path).unwrap_or_default();

    // Only rebuild words.bin if words.bin does not exist or upstream commit changed
    if !words_bin_path.exists()
        || current_sha.trim().is_empty()
        || current_sha.trim() != cached_sha.trim()
    {
        if !build_with_python(&wordlist_dir, &words_bin_path) {
            println!("cargo:warning=Python SCOWL build unavailable or failed; using pure-Rust SCOWL parser fallback.");
            build_with_rust_fallback(&wordlist_dir, &words_bin_path);
        }

        validate_words_bin(&words_bin_path);

        if !current_sha.trim().is_empty() {
            let _ = fs::write(&sha_cache_path, current_sha.trim());
        }
    }
}

fn update_or_clone_scowl(wordlist_dir: &Path, words_bin_path: &Path) {
    if wordlist_dir.join(".git").exists() {
        let fetch_ok = Command::new("git")
            .args(["fetch", "--depth", "1", "origin"])
            .current_dir(wordlist_dir)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);

        if fetch_ok {
            let _ = Command::new("git")
                .args(["reset", "--hard", "FETCH_HEAD"])
                .current_dir(wordlist_dir)
                .status();
        } else if words_bin_path.exists() {
            println!("cargo:warning=Could not fetch latest SCOWL updates (offline?); using cached word list.");
        }
    } else {
        if wordlist_dir.exists() {
            let _ = fs::remove_dir_all(wordlist_dir);
        }
        let status = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                SCOWL_REPO_URL,
                wordlist_dir.to_str().expect("Invalid path"),
            ])
            .status()
            .expect("Failed to execute git clone for SCOWL word list");

        if !status.success() {
            panic!("Failed to clone SCOWL word list from {SCOWL_REPO_URL}");
        }
    }
}

fn get_git_head_sha(wordlist_dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(wordlist_dir)
        .output()
        .ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn build_with_python(wordlist_dir: &Path, words_bin_path: &Path) -> bool {
    let py_script = r#"
import os
import sqlite3
import subprocess
import sys

wordlist_dir = sys.argv[1]
out_bin = sys.argv[2]
db_path = os.path.join(wordlist_dir, "scowl.db")

if os.path.exists(db_path):
    os.remove(db_path)

subprocess.run(
    [sys.executable, "combine.py", "create-db", "--raw", db_path],
    cwd=wordlist_dir,
    check=True,
)

conn = sqlite3.connect(db_path)

# Plurals and -s verb forms (Wordle never uses one as its answer): words SCOWL
# lists only as an -s form, plus words ending in -s/-es (not -ss/-us/-is) whose
# stem is a 3-4 letter noun or verb, since some plurals ("costs") also carry a
# stray noun entry from one of SCOWL's source lists.
inflected_poses = ('ns', 'vs', 'ms', 'nss', 'np', 'nsp', 'nssp', '?', 'x', '')
base_words = {w for (w,) in conn.execute(
    f"SELECT DISTINCT word FROM scowl_v0 WHERE pos NOT IN ({','.join('?' * len(inflected_poses))})",
    inflected_poses,
)}
s_forms = {w for (w,) in conn.execute(
    "SELECT DISTINCT word FROM scowl_v0 WHERE pos IN ('ns', 'vs', 'ms', 'nss')"
)}
stems = {w for (w,) in conn.execute(
    "SELECT DISTINCT word FROM scowl_v0 WHERE pos IN ('n0', 'v0', 'm0') AND length(word) IN (3, 4)"
)}

def is_s_form(w):
    if w in s_forms and w not in base_words:
        return True
    if w.endswith('s') and not w.endswith(('ss', 'us', 'is')):
        return w[:-1] in stems or (w.endswith('es') and w[:-2] in stems)
    return False

rows = conn.execute("""
    SELECT word, MIN(size) as difficulty
    FROM scowl_v0
    WHERE length(word) = 5
      AND word GLOB '[a-z][a-z][a-z][a-z][a-z]'
      AND COALESCE(base_pos, '') NOT IN ('abbr')
      AND COALESCE(pos_category, '') NOT IN ('nonword', 'wordpart')
    GROUP BY word
    ORDER BY word
""").fetchall()
conn.close()

count = 0
with open(out_bin, "wb") as f:
    for word, diff in rows:
        if len(word) == 5 and word.isascii() and word.isalpha() and word.islower():
            b = word.encode("ascii")
            d = min(max(int(diff), 1), 127)
            unique_flag = 0x80 if len(set(word)) == 5 else 0x00
            f.write(b)
            f.write(bytes([d | unique_flag, 0x01 if is_s_form(word) else 0x00]))
            count += 1

if count < 1000:
    raise RuntimeError(f"Unexpectedly few words extracted: {count}")
"#;

    for python_cmd in ["python3", "python"] {
        let status = Command::new(python_cmd)
            .env("PYTHONUTF8", "1")
            .args([
                "-c",
                py_script,
                wordlist_dir.to_str().expect("Invalid wordlist_dir"),
                words_bin_path.to_str().expect("Invalid words_bin_path"),
            ])
            .status();

        if let Ok(s) = status {
            if s.success() && words_bin_path.exists() {
                return true;
            }
        }
    }
    false
}

fn is_five_lowercase_ascii(word: &str) -> bool {
    word.len() == 5 && word.bytes().all(|b| b.is_ascii_lowercase())
}

fn has_unique_bytes(word: &[u8; 5]) -> bool {
    let mut mask: u32 = 0;
    for &b in word {
        let bit = 1u32 << (b - b'a');
        if (mask & bit) != 0 {
            return false;
        }
        mask |= bit;
    }
    true
}

fn build_with_rust_fallback(wordlist_dir: &Path, words_bin_path: &Path) {
    let data_dir = wordlist_dir.join("data");
    let files = [
        "scowl-pre.txt",
        "basic",
        "extra",
        "signature",
        "coca",
        "coca_llm",
        "hacker",
    ];

    let mut words: BTreeMap<[u8; 5], u8> = BTreeMap::new();
    // The Python build's plural sets, read from the entries' part-of-speech tags
    // ("cost <n>: costs") rather than SCOWL's database: 3-4 letter noun/verb/modifier
    // head words, plurals and -s forms, and every other head word or inflection.
    let mut stems: HashSet<String> = HashSet::new();
    let mut s_forms: HashSet<String> = HashSet::new();
    let mut base_words: HashSet<String> = HashSet::new();

    for file_name in files {
        let path = data_dir.join(file_name);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        let mut last_size: u8 = 60;
        for raw_line in content.lines() {
            let line = raw_line.split('#').next().unwrap_or("").trim();
            if line.is_empty() || line.starts_with('-') && !line.contains(':') {
                continue;
            }
            if line.contains("<abbr>")
                || line.contains("/abbr")
                || line.contains("<pre>")
                || line.contains("<suf>")
            {
                continue;
            }

            let Some((left, right)) = line.split_once(':') else {
                continue;
            };

            let size = left
                .split(|c: char| !c.is_ascii_digit())
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<u8>().ok())
                .filter(|&n| (10..=95).contains(&n))
                .min()
                .unwrap_or(last_size);
            last_size = size;

            if let Some((head, tag, inflections)) = parse_entry(right) {
                let pos = tag.split('/').next();
                let noun_verb_mod = matches!(pos, Some("n" | "v" | "m"));
                if head != "-" {
                    if noun_verb_mod
                        && (3..=4).contains(&head.len())
                        && head.bytes().all(|b| b.is_ascii_lowercase())
                    {
                        stems.insert(head.to_string());
                    }
                    base_words.insert(head.to_string());
                }
                // Split on list separators only, so a multi-word form ("color
                // schemes") stays whole and is skipped rather than read as words.
                for form in inflections
                    .split([',', '(', ')', '|'])
                    .map(|t| clean_token(t.trim()))
                    .filter(|t| !t.is_empty() && t.bytes().all(|b| b.is_ascii_lowercase()))
                {
                    // A noun's inflections are all plurals ("cacti" too); a verb's
                    // or modifier's are -s forms only when they end in s.
                    if pos == Some("n") || noun_verb_mod && form.ends_with('s') {
                        s_forms.insert(form.to_string());
                    } else {
                        base_words.insert(form.to_string());
                    }
                }
            }

            for token in right.split(|c: char| {
                c.is_whitespace() || matches!(c, ',' | ':' | ';' | '(' | ')' | '|')
            }) {
                if token.starts_with('<') || token.starts_with('{') || token.starts_with('[') {
                    continue;
                }
                let mut entry_size = size;
                let trimmed = token.trim_end_matches(|c: char| match c {
                    '-' | '@' => {
                        entry_size = entry_size.max(70);
                        true
                    }
                    '~' => {
                        entry_size = entry_size.max(80);
                        true
                    }
                    '!' => {
                        entry_size = entry_size.max(85);
                        true
                    }
                    _ => false,
                });

                if is_five_lowercase_ascii(trimmed) {
                    let mut arr = [0u8; 5];
                    arr.copy_from_slice(trimmed.as_bytes());
                    words
                        .entry(arr)
                        .and_modify(|d| *d = (*d).min(entry_size))
                        .or_insert(entry_size);
                }
            }
        }
    }

    let mut file = fs::File::create(words_bin_path).expect("Failed to create words.bin");
    for (word, diff) in &words {
        let unique_flag = if has_unique_bytes(word) { 0x80 } else { 0x00 };
        let packed = (*diff & 0x7F) | unique_flag;
        file.write_all(word).expect("Failed to write word");
        let w = std::str::from_utf8(word).expect("ASCII word");
        let s_form_only = s_forms.contains(w) && !base_words.contains(w);
        let flags = if s_form_only || looks_plural(w, &stems) {
            0x01
        } else {
            0x00
        };
        file.write_all(&[packed, flags])
            .expect("Failed to write difficulty");
    }
}

/// Splits an entry's text after its sizes ("A Cv: cost <n> {price}: costs") into the
/// head word ("-" when it continues the previous one), its tag ("n", "n/place") and
/// the inflections after the colon that follows the tag, if any. Variant markers
/// ("A Cv:") before the head and a sense note ("{price}") after the tag are skipped.
fn parse_entry(right: &str) -> Option<(&str, &str, &str)> {
    let open = right.find('<')?;
    let close = open + right[open..].find('>')?;
    let head = match right[..open].rsplit(':').next().unwrap_or("").trim() {
        "-" => "-",
        word => clean_token(word),
    };
    let inflections = right[close + 1..].split_once(':').map_or("", |(_, i)| i);
    Some((head, &right[open + 1..close], inflections))
}

/// A word without SCOWL's markers: a leading `@` and trailing `-`, `@`, `~` or `!`.
fn clean_token(token: &str) -> &str {
    token
        .trim_start_matches('@')
        .trim_end_matches(['-', '@', '~', '!'])
}

/// The suffix half of the Python build's plural test: ends in -s/-es (not
/// -ss/-us/-is) after a known 3-4 letter stem.
fn looks_plural(word: &str, stems: &HashSet<String>) -> bool {
    if !word.ends_with('s') || word.ends_with("ss") || word.ends_with("us") || word.ends_with("is")
    {
        return false;
    }
    stems.contains(&word[..word.len() - 1])
        || (word.ends_with("es") && stems.contains(&word[..word.len() - 2]))
}

fn validate_words_bin(words_bin_path: &Path) {
    let bytes = fs::read(words_bin_path).expect("Failed to read generated words.bin");
    assert!(
        !bytes.is_empty() && bytes.len().is_multiple_of(RECORD_SIZE),
        "words.bin length ({}) must be a non-empty multiple of {RECORD_SIZE}",
        bytes.len()
    );
    let count = bytes.len() / RECORD_SIZE;
    assert!(
        count >= 1000,
        "Expected at least 1000 words in words.bin, found {count}"
    );
}
