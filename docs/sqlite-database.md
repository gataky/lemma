# Lemma SQLite Dictionary: Developer Guide

This guide is for developers and coding agents building on `lemma_greek_en.db`, a Modern Greek ↔ English dictionary in a single read-only SQLite file. It covers the schema, the lookup keys your code must compute, ready-to-use queries, and a tested Python reference implementation.

It describes **schema version 4**. Check `meta.schema_version` before use.

## At a glance

| | |
|---|---|
| Greek headwords | 31,615 (33,131 part-of-speech entries, 46,174 senses) |
| Inflection tables | 53,186, holding 489,351 labelled forms (verb conjugations, noun and adjective declensions, and more) |
| Searchable Greek spellings | 366,316 (headwords plus every inflected form: είπα → λέω) |
| English → Greek lookup terms | 88,894 (20,183 point at Greek words or phrases that have no full entry) |
| File size | ~82 MB |
| Source | English Wiktionary via [kaikki.org](https://kaikki.org/) (Greek entries plus English entries' translation tables), inflections from [Dilemma](https://github.com/open-greek/dilemma), word frequencies from [FrequencyWords](https://github.com/hermitdave/FrequencyWords) (OpenSubtitles) |
| Generator | [lemma](https://github.com/open-greek/lemma): `make sqlite` |

Counts are from the 2026-10-30 Wiktionary extraction and change from build to build. Read `meta` for the build you have.

What it is good for:

- **Greek → English** with full detail: part of speech, gender and plural, numbered senses, examples with translations, etymology.
- **Greek spelling help**: type a word without accents (`σπιτι`), misspelled by sound (`σπητι`), or in Greeklish (`spiti`), and get σπίτι.
- **Inflected forms**: `είπα` finds λέω, `δρόμους` finds δρόμος.
- **Paradigms**: every labelled form of a verb, noun or adjective, to draw conjugation and declension tables. See [Inflection tables](#inflection-tables).
- **English → Greek** as a lightweight index: Greek candidates, each with a short English gloss and a sense label to tell meanings apart (bank → τράπεζα [institution], όχθη [edge of river or lake]).

## Rules to follow

1. **Open it read-only** (`file:…?mode=ro`). The database is a build artifact. Never write to it. Keep app data (favourites, history) in your own database.
2. **Do not persist row ids across database versions.** Every build renumbers `headword.id`, `entry.id` and `sense.id`. To remember a word, store `headword.word` (plus `entry.pos` if you need the part of speech) and look it up again.
3. **Your code must compute the lookup keys.** SQL cannot. `form.norm`, `form.phonetic` and `en_index.term` hold keys made by the functions in [Lookup keys](#lookup-keys). Run the same function on the user's input before querying. Use the [Python reference](#python-reference-implementation) as-is, or port it and check your port against its [test vectors](#verifying-a-port).
4. **Check `meta.schema_version` = `'4'`** when opening. A different value means the schema changed, so re-read this guide for that version.

## Schema

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE headword (
    id        INTEGER PRIMARY KEY,
    word      TEXT NOT NULL,
    frequency INTEGER NOT NULL
);
CREATE TABLE entry (
    id          INTEGER PRIMARY KEY,
    headword_id INTEGER NOT NULL REFERENCES headword(id),
    ord         INTEGER NOT NULL,
    pos         TEXT NOT NULL,
    pos_display TEXT NOT NULL,
    head        TEXT,
    etymology   TEXT
);
CREATE TABLE sense (
    id                  INTEGER PRIMARY KEY,
    entry_id            INTEGER NOT NULL REFERENCES entry(id),
    ord                 INTEGER NOT NULL,
    gloss               TEXT NOT NULL,
    example             TEXT,
    example_translation TEXT,
    is_reference        INTEGER NOT NULL
);
CREATE TABLE form (
    norm        TEXT NOT NULL,
    headword_id INTEGER NOT NULL REFERENCES headword(id),
    form        TEXT NOT NULL,
    phonetic    TEXT NOT NULL,
    is_primary  INTEGER NOT NULL,
    PRIMARY KEY (norm, headword_id)
) WITHOUT ROWID;
CREATE TABLE inflection_table (
    id       INTEGER PRIMARY KEY,
    entry_id INTEGER NOT NULL REFERENCES entry(id),
    ord      INTEGER NOT NULL,
    kind     TEXT NOT NULL,
    word     TEXT,
    label    TEXT,
    template TEXT,
    note     TEXT
);
CREATE TABLE inflection_tags (
    id   INTEGER PRIMARY KEY,
    tags TEXT NOT NULL UNIQUE
);
CREATE TABLE inflection (
    table_id INTEGER NOT NULL REFERENCES inflection_table(id),
    ord      INTEGER NOT NULL,
    form     TEXT NOT NULL,
    tags_id  INTEGER NOT NULL REFERENCES inflection_tags(id),
    PRIMARY KEY (table_id, ord)
) WITHOUT ROWID;
CREATE VIEW inflection_row AS
SELECT t.entry_id, t.ord AS table_ord, t.kind, t.word, t.label, t.template, t.note,
       i.ord, i.form, g.tags
FROM inflection_table t
JOIN inflection i ON i.table_id = t.id
JOIN inflection_tags g ON g.id = i.tags_id;
CREATE TABLE en_index (
    term        TEXT NOT NULL,
    greek       TEXT NOT NULL,
    headword_id INTEGER REFERENCES headword(id),
    score       INTEGER NOT NULL,
    sense_id    INTEGER REFERENCES sense(id),
    note        TEXT,
    PRIMARY KEY (term, greek)
) WITHOUT ROWID;
CREATE VIRTUAL TABLE sense_fts USING fts5(
    gloss, content='sense', content_rowid='id', tokenize='porter unicode61'
);
CREATE INDEX entry_headword ON entry(headword_id);
CREATE INDEX sense_entry    ON sense(entry_id);
CREATE INDEX form_phonetic  ON form(phonetic);
CREATE INDEX form_headword  ON form(headword_id);
CREATE INDEX inflection_table_entry ON inflection_table(entry_id);
```

```
headword 1──* entry 1──* sense
    │            │         ▲
    1            1         │ (optional)
    *            *         │
   form  inflection_table  en_index   en_index.headword_id → headword (nullable)
                 1
                 *
            inflection *──1 inflection_tags
```

### `meta`

| key | example |
|---|---|
| `schema_version` | `4` |
| `lemma_version` | `2.0.0` |
| `source` | `English Wiktionary via kaikki.org` |
| `source_lang` | `en` (the definitions are English) |
| `build_date` | `20261105` (YYYYMMDD) |
| `extraction_date` | `2026-10-30` (Wiktionary snapshot date, may be empty) |

### `headword`

One row per distinct Greek spelling.

- `word`: the dictionary form, with accents (σπίτι, λέω, Αθήνα). Case is significant, so `Πόλη` (Constantinople) and `πόλη` (city) are separate headwords.
- `frequency`: raw count in a Greek subtitle corpus (OpenSubtitles 2018). Use it only for ranking, higher meaning more common. `0` means unseen (about 9,600 headwords), not "rare but known".
- Ids run 1..N in dictionary order (Greek alphabetical, ignoring accents; the few entries starting with a digit or symbol, like `2η`, come first). `ORDER BY id` gives a browsable A–Ω list.

### `entry`

One row per part of speech of a headword, in display order (`ord` from 0).

- `pos`: raw Wiktionary part of speech (`noun`, `verb`, `adj`, `name`, …).
- `pos_display`: what to show. Usually the same as `pos`, except verbs whose every sense is a participle become `participle`. Values: `noun`, `adj`, `verb`, `name` (proper noun), `adv`, `participle`, `num`, `intj`, `phrase`, `pron`, `proverb`, `prep`, `conj`, `prep_phrase`, `article`, `det`.
- `head`: grammatical summary for display, or NULL. Examples: `neuter (plural σπίτια)`, `masculine (feminine δέκατη or δεκάτη, neuter δέκατο)`. It is free text, so do not parse it.
- `etymology`: cleaned etymology text, or NULL.

### `sense`

One row per meaning, in order (`ord` from 0). Number them `ord + 1` for display.

- `gloss`: English definition as plain text. It may start with usage labels in parentheses: `(figuratively) house`, `(colloquial) money`, `(obsolete) to eat`. Show it as-is, or style the leading `(…)` as a label.
- `example`, `example_translation`: optional Greek example sentence and its English translation (about 5,500 senses have one). `example_translation` can be NULL even when `example` is set.
- `is_reference`: `1` when the gloss points at another Greek word instead of giving a meaning, e.g. `nominative plural of ελληνικός` or `alternative form of τρώω`. About 6,000 senses. Show these, but when you need one representative meaning for a headword, prefer `is_reference = 0`.

### `form`

Every searchable spelling of every headword. **The headword itself is always included** (`form = headword.word`), plus inflected forms (σπίτια, σπιτιού) and diminutives.

- `norm`: accent- and case-insensitive key (`σπιτιου`). See [`norm`](#norm).
- `phonetic`: sound-based key (`spitiu`). See [`phonetic`](#phonetic).
- `form`: the stored spelling, with accents.
- There is at most one row per (`norm`, headword). Polytonic spellings (ἄνθρωπος) share the `norm` of their monotonic form, so they are found, and `form` normally shows the monotonic spelling (fewer than 200 rows keep a polytonic one).
- One `norm` can belong to several headwords (πόλη and Πόλη both have `norm` πολη), so a lookup can return several rows.
- `is_primary`: `0` when this spelling belongs to several headwords and another one is its main owner. πήγα is a form of both πηγαίνω and πάω, and only πηγαίνω's row has `is_primary = 1`. The main owner is the canonical lemma when [Dilemma](https://github.com/open-greek/dilemma) lists one headword as a variant of the other, and otherwise the more frequent headword. Every other row, including each headword's own spelling, is `1`. Search ranks main owners first; see step 3 below.
- Nothing is capped: a headword has all its inflected forms (verbs can have several hundred) and all their polytonic spellings.

### `en_index`

English term → Greek word or phrase.

- `term`: normalized English (`house`, `living room`, `say`). See [`normalize_term`](#normalize_term). Stored terms are already normalized, so normalize the query once and compare with `=`.
- `greek`: the Greek to show. When `headword_id` is set it equals `headword.word`.
- `headword_id`: the full entry for `greek`, or **NULL** when the Greek came only from an English Wiktionary translation table and has no entry here (θυρωρός, μαύρη λίστα). Show those rows, but they have nothing to tap through to.
- `score`: 1–6, higher is better. See [How `en_index.score` is built](#how-en_indexscore-is-built).
- `sense_id`: the gloss that produced the match, or NULL for translation-only matches.
- `note`: English Wiktionary's label for which sense of the English word this is (`institution`, `edge of river or lake`), or NULL. **This is the best text for telling candidates apart**, so show it.

### `sense_fts`

FTS5 full-text index over `sense.gloss` (external content, rowid = `sense.id`), with the Porter stemmer. Use it as the English fallback when `en_index` has no row, e.g. for phrases like "salt water" or words that only appear inside longer glosses.

## Lookup keys

All of these must be computed in your app, on the user's input, exactly as below. The [Python reference](#python-reference-implementation) is the executable spec. It reproduces the stored `norm` and `phonetic` for all 366,316 `form` rows.

### `norm`

Used for Greek typed without accents or in different case.

1. Unicode NFD-decompose.
2. Drop every combining mark (Unicode general category M*): accents, breathings, diaeresis, iota subscript.
3. Lowercase each character on its own (no context-sensitive final-sigma rule).
4. Replace `ς` with `σ`.

`Σπίτι` → `σπιτι`, `ἄνθρωπος` → `ανθρωποσ`, `ΕΊΠΑ` → `ειπα`.

### `phonetic`

Maps Greek to how it sounds, merging the spellings Modern Greek pronounces alike: ι/η/υ/ει/οι → `i`, ο/ω → `o`, αι/ε → `e`, and doubled letters → single. Output is lowercase Latin plus `8` for θ.

1. NFD-decompose and lowercase as above, keeping two facts per letter: whether it carries an accent (U+0300, U+0301 or U+0342) and whether it carries a diaeresis (U+0308). Drop other marks and fold `ς` → `σ`.
2. Scan left to right, trying in order:
   - **Vowel pairs**, only when the first letter has no accent and the second no diaeresis (τσάι = τσα-ι, κοροϊδεύω = κορο-ι): `ου`→`u`; `ει`, `οι`, `υι`→`i`; `αι`→`e`; `αυ`/`ευ`→`af`/`ef` before θ κ ξ π σ τ φ χ ψ or at the end of the word, `av`/`ev` otherwise (αυτός `aftos`, αύριο `avrio`).
   - **Consonant pairs**: `μπ`→`b`, `ντ`→`d`, `γκ`→`g`, `γγ`→`g`.
   - **Single letters**: α a, β v, γ g, δ d, ε e, ζ z, η i, θ 8, ι i, κ k, λ l, μ m, ν n, ξ ks, ο o, π p, ρ r, σ s, τ t, υ i, φ f, χ h, ψ ps, ω o. Anything else is dropped.
3. Collapse runs of the same character (`ss` → `s`).

`σπίτι` and `σπήτι` → `spiti`; `θάλασσα` → `8alasa`; `πόλη` and `πολύ` → `poli`.

### Greeklish query keys

Latin input is treated as Greeklish and turned into **several** candidate phonetic keys, each with a **penalty** (0 = the most common reading). For example `xeri` gives `heri` (x = χ, penalty 0) and `kseri` (x = ξ, penalty 1). The full table, matched longest first at each position (first matching row wins), is `_GREEKLISH` in the reference code. In short:

- `th`/`8` → θ, `ch` → χ, `ph` → φ, `ps` → ψ, `ks`/`3` → ξ, `ou` → ου, `mp` → μπ, `nt` → ντ.
- `nth` and `mph` are matched before `nt` and `mp`, so `anthropos` is ν+θ.
- Ambiguous letters list their readings most common first: `h` = η or χ, `x` = χ or ξ, `u` = ου or υ, `y` = υ or γ, `b` = μπ or β, `oi`/`ai` = digraph or two vowels.

A candidate's penalty is the sum of its readings' positions. Candidates are capped at 64 (lowest penalty kept), then collapsed and deduplicated.

`query_keys(input)`: if the input contains any Greek letter (U+0370–U+03FF or U+1F00–U+1FFF), it returns exactly `[(phonetic(input), 0)]`. Otherwise it returns the Greeklish candidates.

### `normalize_term`

For English input.

1. Lowercase, split on whitespace.
2. Trim each word of leading and trailing characters that are not letters, digits, `-` or `'`. Drop empty words.
3. If there are 2 or more words and the first is `to`, `a`, `an` or `the`, drop it (`to sleep` → `sleep`).
4. If no letters remain, the result is empty (do not query).
5. Singularize each word: a small irregular table (children→child, people→person, men→man, women→woman, feet→foot, teeth→tooth, mice→mouse, geese→goose, knives→knife, wives→wife, lives→life, leaves→leaf). Otherwise, for ASCII words longer than 3 letters: `-ies`→`-y`, `-sses`→`-ss`, keep `-ss`/`-us`/`-is`, `-shes`/`-ches`/`-xes`/`-zes` drop `es`, else drop a final `s`.
6. Join with single spaces.

`Houses` → `house`, `To Say` → `say`, `living rooms.` → `living room`.

## Searching

### Greek or Greeklish → headwords

This is `search_greek` in the reference code.

1. **If the input contains Greek letters**, query by `norm`:
   ```sql
   SELECT f.headword_id, f.form, h.word, h.frequency, f.phonetic, f.is_primary
   FROM form f JOIN headword h ON h.id = f.headword_id
   WHERE f.norm = :norm
   ORDER BY f.form = h.word DESC;   -- the headword's own spelling first
   ```
   A row is an **exact** match if `f.form` equals the trimmed input. Otherwise it's a **norm** match.
2. **Always**, query by sound with every key from `query_keys`:
   ```sql
   SELECT f.headword_id, f.form, h.word, h.frequency, f.phonetic, f.is_primary
   FROM form f JOIN headword h ON h.id = f.headword_id
   WHERE f.phonetic IN (:k1, :k2, …)
   ORDER BY f.form = h.word DESC;
   ```
   These are **phonetic** matches. Their penalty is the penalty of the key that matched (`f.phonetic`).
3. Keep **one hit per headword**, the best by (kind, penalty, not `is_primary`) with exact < norm < phonetic. On a tie, keep the first row seen.
4. Sort by kind, then penalty, then `is_primary` descending, then `frequency` descending, then `word`. Without `is_primary`, `πήγα` would list πάω first, since it is the more frequent word.

Show `matched_form` when it differs from `word` (`λέω ← είπα`), and label phonetic hits as spelling suggestions ("did you mean").

Examples from this build:

| Input | Top results |
|---|---|
| `σπίτι` | σπίτι (exact) |
| `σπιτι` | σπίτι (norm) |
| `σπητι`, `spiti` | σπίτι (phonetic) |
| `είπα` | λέω ← είπα (exact) |
| `ἄνθρωπος` | άνθρωπος (norm) |
| `xeri` | χέρι, χαίρω ← χαίρει, ξέρω ← ξέρει |
| `poli` | πολύ, Πόλη, πόλη, πολλοί, … |
| `geia` | για, γεια, … (same sound, so frequency decides) |

### Show an entry

```sql
SELECT e.pos_display, e.head, e.etymology,
       s.ord + 1 AS n, s.gloss, s.example, s.example_translation, s.is_reference
FROM entry e JOIN sense s ON s.entry_id = e.id
WHERE e.headword_id = :headword_id
ORDER BY e.ord, s.ord;
```

One representative gloss (for lists and previews):

```sql
SELECT s.gloss FROM entry e JOIN sense s ON s.entry_id = e.id
WHERE e.headword_id = :headword_id
ORDER BY s.is_reference, e.ord, s.ord LIMIT 1;
```

### List a headword's forms

```sql
SELECT form FROM form WHERE headword_id = :headword_id ORDER BY form;
```

This gives σπίτι, σπίτια, σπιτάκι, σπιτιού, σπιτιών, …. Forms here carry no grammatical labels (case, number, tense), so this is a search list. For labelled forms, see [Inflection tables](#inflection-tables).

### Autocomplete

```sql
SELECT DISTINCT h.word
FROM form f JOIN headword h ON h.id = f.headword_id
WHERE f.norm >= :prefix AND f.norm < :prefix || char(1114111)   -- :prefix = norm(typed text)
  AND f.form = h.word                                            -- suggest headwords only
ORDER BY h.frequency DESC
LIMIT 10;
```

Use this range form, which uses the `form` primary key, rather than `LIKE`. Drop the `f.form = h.word` line to also match inflected forms.

### English → Greek

This is `search_english` in the reference code.

```sql
SELECT e.greek, e.headword_id, e.score,
       COALESCE(s.gloss, (
           SELECT s2.gloss FROM entry e2 JOIN sense s2 ON s2.entry_id = e2.id
           WHERE e2.headword_id = e.headword_id
           ORDER BY s2.is_reference, e2.ord, s2.ord LIMIT 1)) AS gloss,
       e.note
FROM en_index e
LEFT JOIN headword h ON h.id = e.headword_id
LEFT JOIN sense s ON s.id = e.sense_id
WHERE e.term = :term                       -- normalize_term(input)
ORDER BY e.score DESC, COALESCE(h.frequency, 0) DESC, e.greek
LIMIT 10;
```

Show `greek`, the `gloss` (shortened), and `note` in brackets. `gloss` is NULL for translation-only rows.

`bank` → τράπεζα `bank [institution]`, όχθη `riverbank, bank [edge of river or lake]`, συστοιχία `array, bank … [row or panel of items]`, …

**Fallback** when that returns nothing. Quote each word so FTS5 syntax in user input is taken literally, and deduplicate by headword:

```sql
SELECT h.word, h.id, s.gloss
FROM sense_fts
JOIN sense s ON s.id = sense_fts.rowid
JOIN entry en ON en.id = s.entry_id
JOIN headword h ON h.id = en.headword_id
WHERE sense_fts MATCH '"salt" "water"'
ORDER BY bm25(sense_fts), h.frequency DESC
LIMIT 50;
```

This gives αλατόνερο `brine, salt water`, άλμη, ….

### How `en_index.score` is built

A gloss contributes 3 if the term stands alone in the gloss's first clause (`city`, `to eat; to chew …`), 2 if it shares the first clause (`house, home`), and 1 if it is in a later clause. Then:

- −1 if the gloss is not the entry's first sense.
- −2 if the gloss is tagged obsolete, archaic, dated, rare or dialectal (never below 1).
- Terms longer than 4 words and reference glosses (`is_reference`) are not indexed.

A matching pair in English Wiktionary's translation tables adds 3, so curated translations outrank incidental gloss matches. Translation-only rows have score 3.

## Inflection tables

`inflection_table` and `inflection` hold Wiktionary's paradigms with their grammatical labels, for every part of speech that has them: conjugations, noun and adjective declensions, pronouns, articles, numerals. Use them to draw a table. To find a word from one of its forms, use `form`, not these.

| | |
|---|---|
| Tables | 53,186 (25,183 inflection tables, 28,003 headword lines) |
| Cells | 489,351 |
| Share of the file | about 20 MB, or 6 MB gzipped |

### Build a table

Start from any spelling. `form` finds the headword, so `βοηθώ` reaches the table on βοηθάω, `σηκώνομαι` the one on σηκώνω and `είπα` the one on λέω:

```sql
SELECT DISTINCT h.id, h.word, e.id AS entry_id, e.pos
FROM form f
JOIN headword h ON h.id = f.headword_id
JOIN entry e ON e.headword_id = h.id
JOIN inflection_table t ON t.entry_id = e.id
WHERE f.norm = :norm AND t.kind IN ('conjugation', 'declension') AND t.word IS NULL;
```

Then read the cells of that entry through the `inflection_row` view:

```sql
SELECT table_ord, kind, label, template, note, form, tags
FROM inflection_row
WHERE entry_id = :entry_id AND word IS NULL
ORDER BY table_ord, ord;
```

Handle one `table_ord` at a time. Then, for each cell of your layout, split `tags` on spaces, match the labels that cell needs, and keep the order of `ord`. Stay within a table because labels are not unique across tables: a positive adjective has no degree label, so `masculine genitive singular` alone also matches the comparative and superlative tables. A word can have several entries (καλός is an adjective and a noun), and `norm` ignores accents (κάλος), so pick by `pos` and spelling.

```js
const hasAll = (tags, wanted) => { const set = new Set(tags.split(" ")); return wanted.every(t => set.has(t)); };
const cell = (rows, ...wanted) => rows.filter(r => hasAll(r.tags, wanted)).map(r => r.form);

// rows: one conjugation table
cell(rows, "active", "past", "perfective", "first-person", "singular");   // ["έγραψα"]
cell(rows, "active", "past", "perfective", "third-person", "plural");     // ["έγραψαν", "γράψαν", "γράψανε"]
```

**Do not match tags with a bare `LIKE`.** `perfective` is inside `imperfective`, and `perfect` is inside both `imperfect` and `perfective`, so `tags LIKE '%perfective%'` returns the wrong tense. In SQL, pad with spaces:

```sql
SELECT form FROM inflection_row
WHERE entry_id = :entry_id AND word IS NULL
  AND ' ' || tags || ' ' LIKE '% perfective %'
  AND ' ' || tags || ' ' LIKE '% past %';
```

A cell can have several rows with identical `tags`. They are alternatives, most usual first (έγραψαν, γράψαν, γράψανε).

### `inflection_table`

- `entry_id`: the part-of-speech entry it belongs to.
- `ord`: order within the entry, from 0.
- `kind`: `conjugation`, `declension`, `inflection` (rare), or `head`. A `head` row holds the forms on the entry's headword line: `plural` for a noun, `feminine` and `neuter` for an adjective, `past` and `passive` for a verb, plus `alternative` spellings. About 2,300 verbs have only this, so it is your source of principal parts when there is no conjugation.
- `word`: NULL for the headword's own tables. Otherwise the table belongs to a different word, one that was merged into this headword: χειρότερος under κακός, καφεδάκι under καφές, τρομαγμένος under τρομάζω. **Filter on `word IS NULL` to get the headword's paradigm.** List the others separately as related words.
- `label`: Wiktionary's label for the whole table, or NULL: `perfect`, `deponent`, `imperfective`, `irregular`.
- `template`: the Wiktionary template that built it, which identifies the inflection class: `el-conjug-1st`, `el-nF-α-ες-3b2`, `el-decl-adj`. Not always informative (`l-self`).
- `note`: text Wiktionary attached to the table, in English, or NULL.

An entry can have several declension or conjugation tables. Adjectives have one each for positive, comparative and superlative. A word with two etymologies that inflect differently has both.

### `inflection`

One row per cell, in Wiktionary's order. `(table_id, ord)` is the key, and `tags_id` points at `inflection_tags`, which stores each distinct label set once. Use the `inflection_row` view rather than joining these yourself.

Unlike `form`, these rows are not deduplicated, have no search keys, and include multi-word forms (θα γράψω, έχοντας γράψει).

### Tags

`tags` is a sorted, space-separated set of Wiktionary labels. There are 681 distinct sets. The ones that matter:

| Part of speech | Labels |
|---|---|
| Person, number | `first-person` `second-person` `third-person`, `singular` `plural` |
| Voice | `active` `passive` |
| Verb aspect | `imperfective` `perfective` |
| Verb tense, mood | `present` `past` `imperfect` `future` `indicative` `imperative` `dependent` `subjunctive` |
| Non-finite | `participle`, `infinitive-aorist` (the απαρέμφατο, γράψει), `indeclinable` |
| Case | `nominative` `genitive` `accusative` `vocative` |
| Gender | `masculine` `feminine` `neuter` |
| Degree | `comparative` `superlative` (neither on a table means positive) |
| Pronoun | `strong` `weak` |
| Article | `definite` `indefinite` |
| Usage | `rare` `informal` `archaic` `dated` `formal` |

How the Modern Greek verb tenses map to tags:

| Tense | Tags |
|---|---|
| Present | `present imperfective indicative` |
| Imperfect | `imperfect imperfective indicative` |
| Simple past | `past perfective indicative` |
| Dependent (να γράψω) | `dependent perfective indicative` |
| Imperative | `imperative imperfective` or `imperative perfective` |

### What Wiktionary does not list

Build these yourself:

- **Future.** Usually only the first person singular is listed (θα γράφω, θα γράψω). For the rest, put θα before the present or dependent forms.
- **Perfect tenses.** Conjugate έχω and add the `infinitive-aorist` form: έχω γράψει, είχα γράψει. Forms with `perfect` in the tags are mostly participles.
- **Subjunctive.** It has no cells of its own. The table's `note` says how to form it.

### Reading the cells

Cell text is Wiktionary's own, unedited, so:

- `-` means the form does not exist, which is different from no row at all (the data just doesn't say). Show it as a dash.
- A row starting with a hyphen is an ending, not a word: γραμμένος, ‑η, ‑ο arrive as three rows with the same tags. Join them for display. Both `-` and the non-breaking `‑` (U+2011) occur.
- In a cell with alternatives, only the first keeps θα: `θα γραφτώ` is followed by bare `γραφώ`.
- About 1,500 cells from hand-built tables hold more than one form or use markup: `παράλυσαν, παραλύσαν`, `{(ε)θεωρείτο}`, `θέλουν² (θένε)`. A few have typos, including Latin letters in Greek words. Keep your display tolerant of them.

The build corrects two problems in the source: Wiktionary's extractor tags comparative and superlative tables identically, and these are told apart (`ομορφότερος` is `comparative`, `ομορφότατος` is `superlative`). Table headers it reports as forms become `note`. Romanizations are dropped, since they can be derived.

## Gotchas

- **Homophones rank by frequency.** `geia` returns για ("for") before γεια ("hello"), and `poli` returns πολύ before πόλη. That is by design for a spelling aid, so show several results with their glosses instead of jumping to the first.
- **Latin input is ambiguous between Greeklish and English.** The database does not guess. Let the user pick the direction, or run both searches and present them separately.
- **Capitalized headwords are separate.** Πόλη/πόλη, Ελλάδα, and names have their own rows. Group them in the UI if you like.
- **`frequency = 0` is common** (~30% of headwords). Treat it as unknown, not as rare.
- **Unlinked English results** (`headword_id IS NULL`) are real translations without a full entry. Multi-word Greek phrases (γεια σας, μαύρη λίστα) appear here too.
- **Wiktionary noise exists.** A few translation pairs are wrong upstream (e.g. eat → εδώ "here"). Ranking keeps them low, and they're not filtered.
- **SQLite's `lower()`/`LIKE` only fold ASCII.** Never rely on them for Greek. Compute `norm` in your code.
- **FTS5 is required for `sense_fts`.** It is in the standard SQLite builds on iOS, macOS, Python and most desktop platforms. Some Android system SQLite builds lack it, so bundle your own SQLite there (e.g. `sqlite-android` or `requery`). Everything except the English fallback works without FTS5.

## Verifying a port

If you reimplement the keys in another language, these must hold.

`norm`: `Σπίτι`→`σπιτι`, `λόγος`→`λογοσ`, `ἄνθρωπος`→`ανθρωποσ`, `Ϊ`→`ι`.

`phonetic`:

| word | key | word | key |
|---|---|---|---|
| σπίτι | spiti | τσάι | tsai |
| σπήτι | spiti | κοροϊδεύω | koroidevo |
| καλημέρα | kalimera | αϋπνία | aipnia |
| θάλασσα | 8alasa | αυτός | aftos |
| παιδί | pedi | αύριο | avrio |
| ούτε | ute | ευχαριστώ | efharisto |
| γεια | gia | Ευρώπη | evropi |
| μπαμπάς | babas | πάντα | pada |
| πόλη, πολύ | poli | | |

Greeklish (`query_keys(q)` must contain `phonetic(word)`): spiti→σπίτι, kalhmera→καλημέρα, thalassa→θάλασσα, 8alasa→θάλασσα, efharisto→ευχαριστώ, eyxaristw→ευχαριστώ, xeri→χέρι, xerw→ξέρω, geia→γεια, yia→γεια, mpampas→μπαμπάς, tsai→τσάι, ellhnika→ελληνικά, Spíti→σπίτι, anthropos→άνθρωπος, nymphi→νύμφη. Penalties: xeri→χέρι 0, xeri→ξέρει 1, kalhmera→καλημέρα 0. `query_keys("σπητι")` = `[("spiti", 0)]`.

`normalize_term`: `To Say`→`say`, `a House`→`house`, `living rooms.`→`living room`, `to`→`to`, `...`→empty. `stem`: houses→house, cities→city, boxes→box, glasses→glass, bus→bus, thing→thing, bed→bed, children→child.

Strongest check: run your `norm` and `phonetic` over every `form.form` and compare with the stored columns. There should be zero differences:

```sql
SELECT form, norm, phonetic FROM form;
```

## Python reference implementation

A single standard-library module (Python 3.8+). It is a port of lemma's Rust code (`src/greek_keys.rs`, `src/english_index.rs`, `src/lookup.rs`), checked against this database: it reproduces the stored `norm` and `phonetic` for all 317,514 forms, and returns the same top-5 results as lemma's `lookup` tool. Save it as `lemma_lookup.py`, or use it as the spec for a port.

```python
from lemma_lookup import open_dictionary, search_greek, search_english, entries

conn = open_dictionary("lemma_greek_en.db")
hit = search_greek(conn, "spiti")[0]          # {'word': 'σπίτι', 'kind': 2, ...}
entries(conn, hit["headword_id"])             # parts of speech with senses
search_english(conn, "bank")[:2]              # τράπεζα [institution], όχθη [...]
```

```python
"""Reference lookup for the lemma SQLite dictionary (schema_version 4).

Port of lemma's src/greek_keys.rs, src/english_index.rs and src/lookup.rs.
Standard library only.
"""

import sqlite3
import unicodedata

# ---------------------------------------------------------------- Greek keys

_ACCENTS = {"̀", "́", "͂"}  # grave, acute, perispomeni
_DIAERESIS = "̈"
_VOICELESS = set("θκξπστφχψ")
_SINGLE = {
    "α": "a", "β": "v", "γ": "g", "δ": "d", "ε": "e", "ζ": "z", "η": "i",
    "θ": "8", "ι": "i", "κ": "k", "λ": "l", "μ": "m", "ν": "n", "ξ": "ks",
    "ο": "o", "π": "p", "ρ": "r", "σ": "s", "τ": "t", "υ": "i", "φ": "f",
    "χ": "h", "ψ": "ps", "ω": "o",
}
_VOWEL_PAIRS = {("ο", "υ"): "u", ("ε", "ι"): "i", ("ο", "ι"): "i",
                ("υ", "ι"): "i", ("α", "ι"): "e"}
_CONSONANT_PAIRS = {("μ", "π"): "b", ("ν", "τ"): "d", ("γ", "κ"): "g",
                    ("γ", "γ"): "g"}

# Greeklish spelling -> readings, most common first (index = penalty).
# Order matters: the first entry that matches at a position wins.
_GREEKLISH = [
    ("nth", ["n8"]), ("mph", ["mf"]),
    ("th", ["8"]), ("ch", ["h"]), ("ph", ["f"]),
    ("ou", ["u"]), ("ei", ["i"]), ("oi", ["i", "oi"]), ("ui", ["i"]),
    ("ai", ["e", "ai"]),
    ("au", ["af", "av"]), ("ay", ["af", "av"]),
    ("eu", ["ef", "ev"]), ("ey", ["ef", "ev"]),
    ("mp", ["b"]), ("nt", ["d"]), ("gk", ["g"]), ("gg", ["g"]),
    ("a", ["a"]), ("b", ["b", "v"]), ("c", ["k"]), ("d", ["d"]),
    ("e", ["e"]), ("f", ["f"]), ("g", ["g"]), ("h", ["i", "h"]),
    ("i", ["i"]), ("j", ["g"]), ("k", ["k"]), ("l", ["l"]), ("m", ["m"]),
    ("n", ["n"]), ("o", ["o"]), ("p", ["p"]), ("q", ["k"]), ("r", ["r"]),
    ("s", ["s"]), ("t", ["t"]), ("u", ["u", "i"]), ("v", ["v"]),
    ("w", ["o"]), ("x", ["h", "ks"]), ("y", ["i", "g"]), ("z", ["z"]),
    ("8", ["8"]), ("3", ["ks"]),
]
MAX_QUERY_KEYS = 64


def _is_mark(c):
    return unicodedata.category(c).startswith("M")


def is_greek(s):
    return any("Ͱ" <= c <= "Ͽ" or "ἀ" <= c <= "῿" for c in s)


def norm(s):
    """Lowercase, strip all diacritics, fold final sigma: Σπίτι -> σπιτι."""
    out = "".join(c.lower() for c in unicodedata.normalize("NFD", s) if not _is_mark(c))
    return out.replace("ς", "σ")


def _collapse(s):
    out = []
    for c in s:
        if not out or out[-1] != c:
            out.append(c)
    return "".join(out)


def _letters(s):
    letters = []  # [base, has_accent, has_diaeresis]
    for c in unicodedata.normalize("NFD", s):
        if c in _ACCENTS:
            if letters:
                letters[-1][1] = True
        elif c == _DIAERESIS:
            if letters:
                letters[-1][2] = True
        elif _is_mark(c):
            continue
        else:
            for lc in c.lower():
                letters.append(["σ" if lc == "ς" else lc, False, False])
    return letters


def phonetic(s):
    """Sound-based key of a Greek word: σπίτι -> spiti, θάλασσα -> 8alasa."""
    letters = _letters(s)
    out = []
    i = 0
    while i < len(letters):
        a, a_accent, _ = letters[i]
        if i + 1 < len(letters):
            b, _, b_diaeresis = letters[i + 1]
            # An accent on the first vowel (τσάι) or a diaeresis on the second
            # (κοροϊδεύω) means two separate vowels, not a digraph.
            if not a_accent and not b_diaeresis:
                pair = _VOWEL_PAIRS.get((a, b))
                if pair:
                    out.append(pair)
                    i += 2
                    continue
                if a in ("α", "ε") and b == "υ":
                    after = letters[i + 2][0] if i + 2 < len(letters) else None
                    out.append("a" if a == "α" else "e")
                    out.append("f" if after is None or after in _VOICELESS else "v")
                    i += 2
                    continue
            pair = _CONSONANT_PAIRS.get((a, b))
            if pair:
                out.append(pair)
                i += 2
                continue
        out.append(_SINGLE.get(a, ""))
        i += 1
    return _collapse("".join(out))


def _greeklish_keys(s):
    latin = "".join(c.lower() for c in unicodedata.normalize("NFD", s) if not _is_mark(c))
    candidates = [("", 0)]
    i = 0
    while i < len(latin):
        match = next((g for g in _GREEKLISH if latin.startswith(g[0], i)), None)
        if match is None:
            i += 1  # not Greeklish: spaces, punctuation
            continue
        spelling, readings = match
        i += len(spelling)
        expanded = [(c + r, p + rank) for c, p in candidates for rank, r in enumerate(readings)]
        expanded.sort(key=lambda kp: kp[1])  # stable
        candidates = expanded[:MAX_QUERY_KEYS]
    keys, seen = [], set()
    for c, p in candidates:
        k = _collapse(c)
        if k and k not in seen:
            seen.add(k)
            keys.append((k, p))
    return keys


def query_keys(s):
    """[(phonetic_key, penalty)] for a Greek or Greeklish query, lowest penalty first."""
    if is_greek(s):
        k = phonetic(s)
        return [(k, 0)] if k else []
    return _greeklish_keys(s)


# -------------------------------------------------------------- English keys

_IRREGULAR_PLURALS = {
    "children": "child", "people": "person", "men": "man", "women": "woman",
    "feet": "foot", "teeth": "tooth", "mice": "mouse", "geese": "goose",
    "knives": "knife", "wives": "wife", "lives": "life", "leaves": "leaf",
}


def stem(w):
    if w in _IRREGULAR_PLURALS:
        return _IRREGULAR_PLURALS[w]
    if len(w) <= 3 or not w.isascii():
        return w
    if w.endswith("ies") and len(w) > 4:
        return w[:-3] + "y"
    if w.endswith("sses"):
        return w[:-2]
    if w.endswith(("ss", "us", "is")):
        return w
    if w.endswith(("shes", "ches", "xes", "zes")):
        return w[:-2]
    if w.endswith("s"):
        return w[:-1]
    return w


def _trim(w):
    keep = lambda c: c.isalnum() or c in "-'"
    start, end = 0, len(w)
    while start < end and not keep(w[start]):
        start += 1
    while end > start and not keep(w[end - 1]):
        end -= 1
    return w[start:end]


def normalize_term(s):
    """English lookup term: 'To Say' -> 'say', 'Houses' -> 'house'."""
    words = [t for t in (_trim(w) for w in s.lower().split()) if t]
    if len(words) > 1 and words[0] in ("to", "a", "an", "the"):
        words = words[1:]
    if not any(c.isalpha() for w in words for c in w):
        return ""
    return " ".join(stem(w) for w in words)


# -------------------------------------------------------------------- search

EXACT, NORM, PHONETIC = 0, 1, 2

_SELECT_FORMS = """
    SELECT f.headword_id, f.form, h.word, h.frequency, f.phonetic, f.is_primary
    FROM form f JOIN headword h ON h.id = f.headword_id"""
_PREFER_HEADWORD = " ORDER BY f.form = h.word DESC"


def search_greek(conn, query, limit=10):
    """Greek or Greeklish query -> [dict], best first.

    Each hit: headword_id, word, matched_form, kind (EXACT/NORM/PHONETIC),
    penalty, is_primary, frequency.
    """
    query = query.strip()
    best = {}

    def rank(h):
        return (h["kind"], h["penalty"], not h["is_primary"])

    def keep(hit):
        cur = best.get(hit["headword_id"])
        if cur is None or rank(hit) < rank(cur):
            best[hit["headword_id"]] = hit

    def hit(row, kind, penalty):
        return {"headword_id": row[0], "matched_form": row[1], "word": row[2],
                "frequency": row[3], "kind": kind, "penalty": penalty,
                "is_primary": bool(row[5])}

    if is_greek(query):
        sql = _SELECT_FORMS + " WHERE f.norm = ?" + _PREFER_HEADWORD
        for row in conn.execute(sql, (norm(query),)):
            keep(hit(row, EXACT if row[1] == query else NORM, 0))

    keys = dict(query_keys(query))
    if keys:
        marks = ", ".join("?" * len(keys))
        sql = _SELECT_FORMS + f" WHERE f.phonetic IN ({marks})" + _PREFER_HEADWORD
        for row in conn.execute(sql, list(keys)):
            keep(hit(row, PHONETIC, keys[row[4]]))

    hits = sorted(best.values(), key=lambda h: (*rank(h), -h["frequency"], h["word"]))
    return hits[:limit]


_SELECT_EN = """
    SELECT e.greek, e.headword_id, e.score,
           COALESCE(s.gloss, (
               SELECT s2.gloss FROM entry e2 JOIN sense s2 ON s2.entry_id = e2.id
               WHERE e2.headword_id = e.headword_id
               ORDER BY s2.is_reference, e2.ord, s2.ord LIMIT 1)),
           e.note
    FROM en_index e
    LEFT JOIN headword h ON h.id = e.headword_id
    LEFT JOIN sense s ON s.id = e.sense_id
    WHERE e.term = ?
    ORDER BY e.score DESC, COALESCE(h.frequency, 0) DESC, e.greek
    LIMIT ?"""

_SELECT_EN_FULL_TEXT = """
    SELECT h.word, h.id, s.gloss
    FROM sense_fts
    JOIN sense s ON s.id = sense_fts.rowid
    JOIN entry en ON en.id = s.entry_id
    JOIN headword h ON h.id = en.headword_id
    WHERE sense_fts MATCH ?
    ORDER BY bm25(sense_fts), h.frequency DESC
    LIMIT ?"""


def search_english(conn, query, limit=10):
    """English query -> [dict], best first.

    Each hit: greek, headword_id (None if the Greek has no entry), score,
    gloss, note (translation sense label), full_text (True if from the
    full-text fallback).
    """
    term = normalize_term(query)
    if not term:
        return []
    hits = [{"greek": r[0], "headword_id": r[1], "score": r[2], "gloss": r[3],
             "note": r[4], "full_text": False}
            for r in conn.execute(_SELECT_EN, (term, limit))]
    if hits:
        return hits

    words = [w.replace('"', "") for w in query.split()]
    match = " ".join(f'"{w}"' for w in words if w)
    if not match:
        return []
    seen = set()
    for word, headword_id, gloss in conn.execute(_SELECT_EN_FULL_TEXT, (match, limit * 5)):
        if headword_id in seen:
            continue
        seen.add(headword_id)
        hits.append({"greek": word, "headword_id": headword_id, "score": 0,
                     "gloss": gloss, "note": None, "full_text": True})
        if len(hits) >= limit:
            break
    return hits


def entries(conn, headword_id):
    """Entries of a headword, each with its senses, in display order."""
    result = []
    for entry_id, pos_display, head, etymology in conn.execute(
            "SELECT id, pos_display, head, etymology FROM entry WHERE headword_id = ? ORDER BY ord",
            (headword_id,)):
        senses = [{"gloss": g, "example": ex, "example_translation": tr, "is_reference": bool(ref)}
                  for g, ex, tr, ref in conn.execute(
                      "SELECT gloss, example, example_translation, is_reference "
                      "FROM sense WHERE entry_id = ? ORDER BY ord", (entry_id,))]
        result.append({"pos_display": pos_display, "head": head,
                       "etymology": etymology, "senses": senses})
    return result


def open_dictionary(path):
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    (version,) = conn.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()
    if version != "4":
        raise RuntimeError(f"expected schema_version 4, got {version}")
    return conn
```

## Rebuilding

The database comes from the lemma repo:

```bash
make sqlite      # writes lemma_greek_en/lemma_greek_en.db
```

`make sqlite` reads the Dilemma files and the English kaikki dump from `../lemma-data` and stops if one is missing, so you get the numbers above. Running `cargo run --release` without that data still works, with fewer inflected forms (about 257K) and an English index built from glosses only. See the lemma README.

## License

Dictionary content is derived from Wiktionary and licensed [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/), which requires attribution and that redistributed or adapted data stays under the same license. Credit Wiktionary (and kaikki.org) in any app that ships this database. Frequency data comes from FrequencyWords (MIT). The lemma generator code is MIT.
