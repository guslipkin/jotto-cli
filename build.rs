use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCOWL_REPO_URL: &str = "https://github.com/en-wl/wordlist.git";
const BUILD_FORMAT_VERSION: &str = "v2-pure-r1";

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
        if len(word) == 5 and len(set(word)) == 5 and word.isascii() and word.isalpha() and word.islower():
            b = word.encode("ascii")
            d = min(max(int(diff), 1), 255)
            f.write(b)
            f.write(bytes([d]))
            count += 1

if count < 1000:
    raise RuntimeError(f"Unexpectedly few words extracted: {count}")
"#;

    for python_cmd in ["python3", "python"] {
        let status = Command::new(python_cmd)
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

fn has_five_unique_lowercase_ascii(word: &str) -> bool {
    if word.len() != 5 {
        return false;
    }
    let mut mask: u32 = 0;
    for &b in word.as_bytes() {
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

                if has_five_unique_lowercase_ascii(trimmed) {
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
        file.write_all(word).expect("Failed to write word");
        file.write_all(&[*diff])
            .expect("Failed to write difficulty");
    }
}

fn validate_words_bin(words_bin_path: &Path) {
    let bytes = fs::read(words_bin_path).expect("Failed to read generated words.bin");
    assert!(
        !bytes.is_empty() && bytes.len().is_multiple_of(6),
        "words.bin length ({}) must be a non-empty multiple of 6",
        bytes.len()
    );
    let count = bytes.len() / 6;
    assert!(
        count >= 1000,
        "Expected at least 1000 words in words.bin, found {count}"
    );
}
