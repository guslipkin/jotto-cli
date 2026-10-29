#!/usr/bin/env python3
"""Build data/concepts.bin, the word list and meaning vectors `concepts` embeds.

Run by hand when the word lists or vectors should change, not from build.rs:
GloVe's archive is 822 MB, far too much to fetch on every build, while the
result is under 20 MB and is committed.

    python3 scripts/build-concepts-data.py \
        --scowl-db target/release/build/jotto-*/out/wordlist/scowl.db \
        --glove glove.6B.300d.txt \
        --wordnet wordnet

The SCOWL database is the one build.rs creates (any `cargo build` leaves it in
OUT_DIR). The vectors are GloVe 6B (Wikipedia 2014 + Gigaword 5, Public Domain
Dedication and License), from https://nlp.stanford.edu/projects/glove/ or
https://huggingface.co/stanfordnlp/glove. GloVe was chosen over fastText and
word2vec because its license puts no conditions on the MIT-licensed binary.
Use the 300-dimension file: the 100-dimension one makes a file a third the
size, but ranks the middle of the list (#100-#2,000) noticeably worse.

WordNet 3.0 (the `wordnet` directory of its dict files, from
https://wordnet.princeton.edu or NLTK's data repository's
packages/corpora/wordnet.zip) sorts the secret words into difficulty tiers by
the category of each one's most common sense: things you can point at are
easy, acts and events medium, ideas and qualities hard. Word frequency was
tried first and backfires: GloVe's most frequent nouns are abstract news words
("investigation", "regime"), the hardest to find by meaning. Only the tier
number is written, none of WordNet itself.

Names, places and brands ("amazon", "google", "colin", "paris") are left out of
the game entirely, listed in data/concepts-proper-nouns.txt. GloVe 6B is
lowercased, so the list comes from a cased vocabulary: with `--cased-vocab`
(the first column of GloVe 840B's glove.840B.300d.txt, from the same pages; a
2 GB download, so only needed to refresh the list), a word is a proper noun
when its capitalized form is more than PROPER_NOUN_RATIO times as frequent as
its lowercase one, SCOWL's lists of names, surnames or towns have it
capitalized, and WordNet has no
lowercase sense of it that its hand-tagged corpus ever used. The ratio alone
would drop common nouns used in names ("university", "creek"); the WordNet
check keeps real words with a name sense ("wed", "frank"). PROPER_NOUN_KEEP
and PROPER_NOUN_DROP correct the cases no rule gets right.

`--show WORD ...` prints each word's nearest neighbours as the game would rank
them, from the quantized vectors just written, to check a change's effect.

File format (little-endian):
    b"CNCP", u8 version, u8 reserved, u16 dim,
    u32 length + newline-separated sorted guessable words,
    u32 length + newline-separated "form lemma" aliases sorted by form,
    u32 length + newline-separated "word tier" secret words (tier 1 easy,
        2 medium, 3 hard; each tier's pool includes the easier ones),
    n_words bytes, 1 for a ranked (core) word and 0 for a guess-only one,
    n_words * dim int8 vectors, in word order.
"""

import argparse
import re
import sqlite3
import struct
import sys
from pathlib import Path

FORMAT_VERSION = 2

# SCOWL parts of speech a guess may be (base forms only). Function words
# (determiners, pronouns, prepositions, conjunctions) aren't guessable: they
# sit close to everything in GloVe and would only clutter the ranking.
CONTENT_POSES = ("n0", "v0", "aj0", "av0", "a0", "m0")
FUNCTION_BASE_POSES = ("d", "pn", "pp", "c", "s", "i")

# Ranked (core) words: SCOWL's medium list, among GloVe's most frequent words.
# Ranks, hints and the closest-words list come only from these.
CORE_MAX_SIZE = 50
CORE_MAX_GLOVE_RANK = 100_000

# Guess-only words: every other SCOWL word GloVe has (its whole 400,000-word
# vocabulary). They're accepted as guesses and ranked by where they'd fall
# among the core words, but never fill a rank of their own: past size 50 SCOWL
# brings in names ("ginny", "colin") and rare words whose vectors are noisy,
# which would otherwise turn up among a secret's closest words and in hints.
# Kept out of sight that way, they cost only file size (300 bytes a word).
GUESS_MAX_SIZE = 95
GUESS_MAX_GLOVE_RANK = 400_000

# Secret words: common nouns only, so every secret is something a player
# could reasonably name.
SECRET_MAX_SIZE = 35
SECRET_MAX_GLOVE_RANK = 12_000
SECRET_MIN_LENGTH = 3

# WordNet noun categories (lexicographer files) by difficulty tier; any other
# category, or a word WordNet doesn't have, is tier 3 (hard).
EASY_CATEGORIES = {
    "noun.artifact", "noun.person", "noun.location", "noun.object",
    "noun.substance", "noun.animal", "noun.body", "noun.food", "noun.plant",
}
MEDIUM_CATEGORIES = {
    "noun.act", "noun.event", "noun.group", "noun.time", "noun.phenomenon",
    "noun.process", "noun.shape",
}

# A noun that's also one of these, in SCOWL's medium list, is mostly something
# else ("southern", "cool", "won", "hiding") and isn't a secret.
NON_NOUN_POSES = ("aj0", "av0", "a0", "vd", "vn", "vg")
NON_NOUN_MAX_SIZE = 50

# Inflections of any SCOWL size map to their base word.
ALIAS_MAX_SIZE = 95

# Never a secret word, though still guessable: slurs, profanity and
# subjects that make an unpleasant daily puzzle.
SECRET_BLOCKLIST = {
    "abortion", "abuse", "anus", "ass", "assassination", "bastard", "bitch",
    "bloodshed", "bomb", "breast", "butt", "cancer", "corpse", "crap", "cunt",
    "damn", "death", "dick", "drug", "fag", "fuck", "genocide", "gun",
    "gunfire", "gunman", "hell", "holocaust", "homicide", "homosexuality",
    "kill", "killer", "massacre", "murder", "nazi", "negro", "nigger", "penis",
    "piss", "porn", "pornography", "prostitute", "prostitution", "rape",
    "rapist", "sex", "sexuality", "shit", "slaughter", "slave", "slavery",
    "slut", "sniper", "suicide", "terror", "terrorism", "terrorist",
    "torture", "vagina", "victim", "war", "whore",
}

# Nouns in SCOWL that people mostly use as something else, found by reading
# the generated list: verbs, numbers, names and adjectives. SCOWL ranks both
# senses alike, so no rule above can tell them apart.
SECRET_NOT_NOUNS = {
    "affect", "amp", "are", "beloved", "best", "better", "billion", "bob",
    "bologna", "can", "carry", "chi", "chile", "chuck", "cite", "compare",
    "concentrate", "cooper", "cope", "cox", "damages", "die", "dig", "dole",
    "don", "eight", "eighteen", "eighth", "eleven", "employ", "expose",
    "fail", "feel", "fifteen", "fifth", "fifty", "fill", "find", "first",
    "five", "folks", "ford", "forensic", "forthcoming", "forty", "four",
    "fourteen", "fourth", "fuller", "gather", "get", "give", "glen", "gore",
    "hang", "hart", "have", "heath", "humanitarian", "hundred", "invite",
    "jay", "join", "keep", "ken", "latest", "lie", "listen", "look", "make",
    "marshall", "mart", "martin", "mike", "million", "moo", "newton", "nick",
    "nine", "ninth", "nova", "peter", "polish", "potter", "regards",
    "reject", "remove", "resist", "rouge", "sally", "see", "sell", "serve",
    "settle", "seven", "seventh", "sing", "singles", "six", "sixteen",
    "sixth", "sixty", "smith", "steal", "sub", "take", "tang", "ten", "tenth",
    "third", "thirteen", "thirty", "thousand", "three", "transform",
    "trillion", "trump", "try", "turner", "twelve", "twenty", "two", "wade",
    "want", "warren", "weigh", "will", "works", "worse", "worst", "wright",
    "yen", "zero",
    # Common nouns whose GloVe vector is mostly a name's, so their closest
    # words would be about the person, place or institution.
    "bush", "fisher", "march", "miller", "pentagon", "phoenix",
}

WORD_RE = re.compile(r"^[a-z]+$")


# Proper nouns (see the docstring): how many times more often the capitalized
# form must appear in the cased vocabulary than the lowercase one.
PROPER_NOUN_RATIO = 5

# Caught by the proper-noun rule but words in their own right, found by reading
# the whole list it writes: common nouns that are also names ("robin"), words
# from names that are ordinary vocabulary (peoples and faiths, geological
# periods, eponyms like "stetson" and "wellington", units like "henry"), and
# trades that are also surnames ("fletcher").
PROPER_NOUN_KEEP = {
    "anabaptist", "apollonian", "argonaut", "baedeker", "bailey", "bastille",
    "ben", "benedictine", "bolshevik", "bolshevist", "boniface", "brant",
    "breton", "broadway", "buddha", "caerphilly", "calvary", "carter",
    "cartwright", "catawba", "catholicism", "cenozoic", "cepheid", "chapman",
    "chartism", "chautauqua", "cheshire", "chesterfield", "cheviot", "colby",
    "cole", "cretaceous", "dalton", "delphic", "devonian", "dobbin", "domesday",
    "duroc", "dutch", "egyptian", "english", "episcopalian", "fagin",
    "faustian", "fermi", "flemish", "fletcher", "ford", "fowler", "french",
    "german", "germanic", "gethsemane", "gilbert", "glover", "golgotha",
    "greek", "guarani", "guernsey", "harper", "hellenization", "henry",
    "holstein", "jacobin", "jeroboam", "jesuit", "jezebel", "kalashnikov",
    "knesset", "knickerbocker", "kremlin", "kwakiutl", "lenten", "lollard",
    "macon", "mameluke", "manhattan", "maxwell", "mediterranean", "melton",
    "methodism", "methodist", "methuselah", "micawber", "montgolfier",
    "morris", "nebuchadnezzar", "neogene", "oxford", "palmer", "panama",
    "pawnee", "pharisee", "pickwick", "piedmont", "presbyterian",
    "promethean", "proterozoic", "pullman", "quixote", "risorgimento",
    "rubaiyat", "sargasso", "saskatoon", "savoy", "saxony", "shylock",
    "silurian", "skinner", "slater", "sodom", "spencer", "stetson",
    "tamworth", "tartuffe", "tatar", "timon", "torah", "triassic", "ulster",
    "unitarian", "vulgate", "waterloo", "weber", "webster", "wellington",
    "winesap",
    "antarctic", "auburn", "august", "baptist", "barker", "beck", "bowman",
    "brogan", "busby", "carol", "catholic", "chad", "chandler", "clement",
    "collier", "cooper", "cox", "dale", "davenport", "farrow", "flagstaff",
    "glen", "griffin", "hart", "hector", "jay", "josh", "lee", "llano",
    "marshall", "martin", "midland", "miller", "newton", "noel", "pacific",
    "pentagon", "phoebe", "phoenix", "pippin", "randy", "rick", "robin",
    "roman", "sawyer", "smith", "soviet", "terry", "thatcher", "tom", "tony",
    "troy", "tucker", "turner", "waggoner", "wagoner", "warren", "welch",
    "welsh", "woodbine", "wright",
}

# Names the rule misses because WordNet has a rarely used lowercase sense of
# them ("john", a toilet; "mike", a microphone): players would only type them
# as names, and their vectors are the names'.
PROPER_NOUN_DROP = {
    "billy", "bobby", "bonnie", "colleen", "garbo", "hank", "harry", "jimmy",
    "john", "ken", "maria", "matt", "mike", "morocco", "sanger", "waldo",
}

DEFAULT_PROPER_NOUNS = Path(__file__).resolve().parent.parent / "data" / "concepts-proper-nouns.txt"


def parse_args():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--scowl-db", required=True, type=Path)
    p.add_argument("--glove", required=True, type=Path)
    p.add_argument("--wordnet", required=True, type=Path, help="WordNet 3.0 dict directory")
    p.add_argument("--cased-vocab", type=Path, help="GloVe 840B (or just its first column): rewrite the proper-noun list")
    p.add_argument("--proper-nouns", type=Path, default=DEFAULT_PROPER_NOUNS)
    p.add_argument("--out", type=Path, default=Path(__file__).resolve().parent.parent / "data" / "concepts.bin")
    p.add_argument("--show", nargs="*", default=[], metavar="WORD")
    return p.parse_args()


def scowl_lists(db_path):
    conn = sqlite3.connect(db_path)
    sizes = {}  # base-form word -> smallest SCOWL size as a content word
    nouns = {}  # base-form noun -> smallest SCOWL size
    for word, pos, size in conn.execute(
        f"SELECT word, pos, MIN(size) FROM scowl_v0 WHERE pos IN ({','.join('?' * len(CONTENT_POSES))}) GROUP BY word, pos",
        CONTENT_POSES,
    ):
        if not WORD_RE.match(word):
            continue
        sizes[word] = min(size, sizes.get(word, size))
        if pos == "n0":
            nouns[word] = min(size, nouns.get(word, size))
    function_words = {
        w for (w,) in conn.execute(
            f"SELECT DISTINCT word FROM scowl_v0 WHERE base_pos IN ({','.join('?' * len(FUNCTION_BASE_POSES))})",
            FUNCTION_BASE_POSES,
        )
    }
    other_poses = {
        w for (w,) in conn.execute(
            f"SELECT DISTINCT word FROM scowl_v0 WHERE pos IN ({','.join('?' * len(NON_NOUN_POSES))}) AND size <= ?",
            (*NON_NOUN_POSES, NON_NOUN_MAX_SIZE),
        )
    }
    # Only SCOWL's lists of names, surnames and towns: a word that's merely
    # spelled capitalized ("Americanization", "Anglophobia") isn't a name.
    capitalized = {
        w.lower() for (w,) in conn.execute(
            "SELECT DISTINCT word FROM scowl_v0 WHERE tag IN ('[name]', '[census]', '[town]')"
        )
        if w[:1].isupper() and w[1:].islower()
    }
    inflections = {}  # form -> set of lemmas
    for form, lemma in conn.execute(
        "SELECT DISTINCT e.word, e.lemma FROM entries e JOIN scowl_v0 s ON s.word = e.word "
        "WHERE e.word != e.lemma AND s.size <= ?",
        (ALIAS_MAX_SIZE,),
    ):
        if WORD_RE.match(form) and lemma and WORD_RE.match(lemma):
            inflections.setdefault(form, set()).add(lemma)
    conn.close()
    return sizes, nouns, function_words, other_poses, capitalized, inflections


def wordnet_tiers(wn_dir):
    """Returns {noun: tier} from the category of each noun's first (most common) sense."""
    lexnames = {}
    for line in open(wn_dir / "lexnames", encoding="ascii"):
        num, name, _ = line.split()
        lexnames[int(num)] = name
    synset_category = {}
    for line in open(wn_dir / "data.noun", encoding="latin-1"):
        if not line.startswith("  "):  # the license header lines start with two spaces
            offset, lex_filenum = line.split(maxsplit=2)[:2]
            synset_category[offset] = lexnames[int(lex_filenum)]
    tiers = {}
    for line in open(wn_dir / "index.noun", encoding="latin-1"):
        if line.startswith("  "):
            continue
        # lemma pos synset_cnt p_cnt [ptr_symbol...] sense_cnt tagsense_cnt synset_offset...
        fields = line.split()
        p_cnt = int(fields[3])
        first_offset = fields[4 + p_cnt + 2]
        category = synset_category[first_offset]
        tiers[fields[0]] = 1 if category in EASY_CATEGORIES else 2 if category in MEDIUM_CATEGORIES else 3
    return tiers


def wordnet_lowercase_attested(wn_dir):
    """Lemmas with a lowercase sense that WordNet's hand-tagged corpus used at least once."""
    pos_files = {"1": "noun", "2": "verb", "3": "adj", "4": "adv", "5": "adj"}
    synset_words = {}
    for pos in set(pos_files.values()):
        for line in open(wn_dir / f"data.{pos}", encoding="latin-1"):
            if line.startswith("  "):
                continue
            fields = line.split()
            count = int(fields[3], 16)
            # Adjective forms can carry a marker like "(a)"; drop it.
            synset_words[(pos, fields[0])] = [fields[4 + 2 * i].split("(")[0] for i in range(count)]
    attested = set()
    for line in open(wn_dir / "index.sense", encoding="latin-1"):
        key, offset, _, tag_count = line.split()
        if int(tag_count) == 0:
            continue
        lemma, rest = key.split("%")
        for form in synset_words.get((pos_files[rest[0]], offset), []):
            if form.lower() == lemma and form.islower():
                attested.add(lemma)
    return attested


def find_proper_nouns(words, cased_vocab, capitalized, attested):
    """Words used mostly capitalized (see the docstring), from a rank-ordered cased vocabulary."""
    forms = {}
    for w in words:
        for form in (w, w.capitalize(), w.upper()):
            forms[form] = w
    rank = {}
    with open(cased_vocab, encoding="utf-8", errors="replace") as f:
        for i, line in enumerate(f):
            token = line.split(" ", 1)[0].rstrip("\n")
            if token in forms and token not in rank:
                rank[token] = i + 1
    missing = 10**9
    found = set()
    for w in words:
        # Ranks are frequency-ordered, so frequency ~ 1/rank.
        lower = rank.get(w, missing)
        cased = min(rank.get(w.capitalize(), missing), rank.get(w.upper(), missing))
        if lower / cased > PROPER_NOUN_RATIO and w in capitalized and w not in attested:
            found.add(w)
    return (found - PROPER_NOUN_KEEP) | (PROPER_NOUN_DROP & set(words))


def read_glove(path, wanted, max_rank):
    """Returns {word: (rank, vector)} for wanted words among GloVe's first max_rank."""
    found = {}
    with open(path, encoding="utf-8") as f:
        for rank, line in enumerate(f):
            if rank >= max_rank:
                break
            word, _, rest = line.partition(" ")
            if word in wanted:
                found[word] = (rank, [float(x) for x in rest.split()])
    return found


def quantize(vectors, centre_on):
    """Mean-centres, normalises and scales each vector to int8.

    Centring removes the direction every GloVe vector shares, which otherwise
    makes frequent words look close to everything. The mean is the ranked
    words' only: guess-only words outnumber them and are mostly rare, and
    their mean pulls common words ("actually", "dad") in among a secret's
    closest words.
    """
    dim = len(next(iter(vectors.values())))
    n = len(centre_on)
    mean = [sum(vectors[w][i] for w in centre_on) / n for i in range(dim)]
    out = {}
    for word, v in vectors.items():
        c = [x - m for x, m in zip(v, mean)]
        peak = max(abs(x) for x in c) or 1.0
        out[word] = bytes((round(x / peak * 127) & 0xFF) for x in c)
    return dim, out


def write_blob(f, lines):
    data = "\n".join(lines).encode("ascii")
    f.write(struct.pack("<I", len(data)))
    f.write(data)


def signed(b):
    return [x - 256 if x > 127 else x for x in b]


def show_neighbours(words, qvec, targets, top=25):
    ints = {w: signed(qvec[w]) for w in words}
    norms = {w: sum(x * x for x in v) ** 0.5 for w, v in ints.items()}
    for t in targets:
        if t not in ints:
            print(f"{t}: not guessable")
            continue
        tv, tn = ints[t], norms[t]
        sims = sorted(
            ((sum(a * b for a, b in zip(tv, v)) / (tn * norms[w]), w) for w, v in ints.items()),
            reverse=True,
        )
        ranked = [w for _, w in sims]
        print(f"{t}: {' '.join(ranked[1:top + 1])}")
        print(f"    rank 100: {ranked[99]}  500: {ranked[499]}  2000: {ranked[1999]}  10000: {ranked[9999]}")


def main():
    args = parse_args()
    sizes, nouns, function_words, other_poses, capitalized, inflections = scowl_lists(args.scowl_db)

    candidates = {w for w, s in sizes.items() if s <= GUESS_MAX_SIZE and w not in function_words and len(w) >= 2}
    glove = read_glove(args.glove, candidates, GUESS_MAX_GLOVE_RANK)

    if args.cased_vocab:
        proper = find_proper_nouns(
            glove, args.cased_vocab, capitalized, wordnet_lowercase_attested(args.wordnet)
        )
        args.proper_nouns.write_text(
            "# Left out of concepts: names, places and brands. Written by\n"
            "# scripts/build-concepts-data.py --cased-vocab (its docstring says how);\n"
            "# edit PROPER_NOUN_KEEP / PROPER_NOUN_DROP there rather than this file.\n"
            + "".join(f"{w}\n" for w in sorted(proper))
        )
    proper = {
        line.strip() for line in args.proper_nouns.read_text().splitlines()
        if line.strip() and not line.startswith("#")
    }
    glove = {w: v for w, v in glove.items() if w not in proper}
    core = {w for w in glove if sizes[w] <= CORE_MAX_SIZE and glove[w][0] < CORE_MAX_GLOVE_RANK}
    # A guess-only word that's also an inflection of a core word ("stained",
    # "reinforced") is scored as that word, like any other inflection.
    words = sorted(
        w for w in glove
        if w in core or not any(l in core for l in inflections.get(w, ()))
    )

    word_set = set(words)
    aliases = []
    for form in sorted(inflections):
        if form in word_set:
            continue
        lemmas = [l for l in inflections[form] if l in word_set]
        # Prefer a core lemma, so "stained" is "stain" even though "stained" is a word.
        lemmas = [l for l in lemmas if l in core] or lemmas
        if lemmas:
            # An ambiguous form ("left": leave/left) maps to its most frequent lemma.
            aliases.append(f"{form} {min(lemmas, key=lambda l: glove[l][0])}")

    secrets = sorted(
        w for w in words
        if w in core
        and nouns.get(w, 99) <= SECRET_MAX_SIZE
        and glove[w][0] < SECRET_MAX_GLOVE_RANK
        and len(w) >= SECRET_MIN_LENGTH
        and w not in other_poses
        and w not in SECRET_BLOCKLIST
        and w not in SECRET_NOT_NOUNS
    )

    tiers = wordnet_tiers(args.wordnet)

    dim, qvec = quantize({w: glove[w][1] for w in words}, sorted(core))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(b"CNCP")
        f.write(struct.pack("<BBH", FORMAT_VERSION, 0, dim))
        write_blob(f, words)
        write_blob(f, aliases)
        write_blob(f, [f"{w} {tiers.get(w, 3)}" for w in secrets])
        f.write(bytes(1 if w in core else 0 for w in words))
        for w in words:
            f.write(qvec[w])

    print(
        f"Wrote {args.out}: {len(proper)} proper nouns left out, {len(words)} words ({len(core)} ranked), {len(aliases)} aliases, "
        f"{len(secrets)} secrets ({', '.join(f'{sum(1 for w in secrets if tiers.get(w, 3) <= n)} tier {n}' for n in (1, 2, 3))}), {dim} dimensions, {args.out.stat().st_size:,} bytes",
        file=sys.stderr,
    )
    if args.show:
        show_neighbours(sorted(core), qvec, args.show)


if __name__ == "__main__":
    main()
