# Lemma: Modern Greek-English Dictionary Database

A free Modern Greek-English dictionary as a single SQLite file, built from Wiktionary data for apps and ad-hoc queries. It has 31K headwords, 366K searchable spellings (accent-insensitive, by sound, or in Greeklish), English → Greek lookup, and 53K labelled inflection tables for drawing verb, noun and adjective paradigms. The generator and all helper tools are written in Rust.

Lemma used to build Kindle (MOBI/EPUB) and StarDict dictionaries. Those outputs have been removed; earlier releases on the [Releases page](https://github.com/open-greek/lemma/releases) still have them.

## Building

### Prerequisites

- Rust 1.85+ (edition 2024)
- The data in a `lemma-data` directory next to this repo:
  - `kaikki/en-el/kaikki.org-dictionary-Greek-words.jsonl` (Greek entries from English Wiktionary)
  - `kaikki/en/kaikki.org-dictionary-English.jsonl.gz` (English entries, for their translation tables)
  - `dilemma/mg_lookup_scored.json` and `dilemma/mg_ranked_forms.json` (from [Dilemma](https://github.com/open-greek/dilemma))

### Build the database

```bash
make sqlite                                 # writes lemma_greek_en/lemma_greek_en.db
make sqlite LEMMA_DATA=/path/to/lemma-data  # data somewhere else
```

`make sqlite` stops if a data file is missing. Running `cargo run --release` directly also works without the data, but gives a smaller dictionary: fewer inflected forms, and an English index built from glosses alone.

Without a local Greek dump, the build downloads it from kaikki.org to `greek_data_en.jsonl` and reuses that file on later runs. Delete it to download a fresh one.

### Command Line Arguments

- `-l, --limit PERCENT`: Limit to the first X% of words (for testing). Writes to `lemma_greek_en_<X>pct/`
- `-s, --source en|el`: English (default) or Greek Wiktionary
- `-h, --help`: Show help message

## The Database

The database holds the processed dictionary (after form-of merging and inflection deduplication) as plain tables. For using the database from another project (schema details, how to compute the lookup keys, queries, and a tested Python reference implementation), see [docs/sqlite-database.md](docs/sqlite-database.md).

- `headword` (word, corpus frequency), `entry` (one per part of speech, with gender/plural head info and etymology), `sense` (gloss plus optional example and translation)
- `form`: every searchable spelling of a headword, including inflected forms (είπα → λέω), with two lookup keys: `norm` (accent- and case-insensitive, `σπιτι`) and `phonetic` (by sound, so `σπητι`, `spiti` and σπίτι share the key `spiti`)
- `inflection_table`, `inflection`: labelled paradigms for conjugation and declension tables (`έγραψα`: `active first-person indicative past perfective singular`), for verbs, nouns, adjectives and every other part of speech Wiktionary inflects. Unlike `form`, these keep tense, person, case, gender and number, and include multi-word forms (θα γράψω)
- `en_index`: English → Greek. English terms taken from the Greek entries' glosses, plus (optionally) English Wiktionary's translation tables, scored so curated translations and a gloss's leading words rank first. Translation-table words with no Greek entry of their own (μαύρη λίστα, "blacklist") are kept as text with no `headword_id`
- `sense_fts`: FTS5 full-text index over glosses, used when `en_index` has no match
- `meta`: schema version, lemma version, source, and Wiktionary extraction date

For the translation tables, download the English kaikki dump (~534 MB gzipped) to `$KAIKKI_LOCAL_DIR/en/kaikki.org-dictionary-English.jsonl.gz`:

```bash
curl -L --fail -R --create-dirs \
  -o "$KAIKKI_LOCAL_DIR/en/kaikki.org-dictionary-English.jsonl.gz" \
  https://kaikki.org/dictionary/English/kaikki.org-dictionary-English.jsonl.gz
```

It is streamed without unpacking (about 7 s). Without it, `en_index` is built from glosses alone.

The `lookup` tool queries the database, accepting Greek (accents optional, misspellings tolerated), Greeklish, or with `--en` English:

```bash
cargo run --release --bin lookup -- spiti
cargo run --release --bin lookup -- --en bank
cargo run --release --bin lookup                 # interactive
cargo run --release --bin lookup -- --check tests/golden_queries.tsv
```

`tests/golden_queries.tsv` lists queries and the headword each should return (`en:` marks English queries); CI runs it against every build. `tests/golden_queries_translations.tsv` holds English queries that need the translation tables, so run it locally after a build with the English dump.

## Data Sources

The dictionary is built from:

- **Primary Source**: [Kaikki.org](https://kaikki.org/) - Machine-readable Wiktionary data (definitions, POS, etymology)
- **Inflection Data** (optional): [Dilemma](https://github.com/open-greek/dilemma) - Greek lemmatizer with 2.74M Modern Greek form-to-lemma mappings compiled from English and Greek Wiktionary, treebank corpora, and LSJ expansion
- **Ranked Inflections** (optional): Dilemma's `mg_ranked_forms.json` from the [`open-greek/dilemma-data`](https://huggingface.co/datasets/open-greek/dilemma-data) HuggingFace dataset provides pre-ranked, case-deduplicated inflection lists per lemma. Download it into `lemma-data/dilemma/`.
- **Frequency Data** (fallback): [FrequencyWords](https://github.com/hermitdave/FrequencyWords) - Word frequency lists derived from OpenSubtitles 2018 corpus, used to rank inflections when pre-ranked forms are not available

### Optional Configuration

`make sqlite` points the build at `lemma-data`. To run `cargo run` directly with local kaikki dumps or Dilemma inflection data, set these in the environment or in a `.env` file in the project root:

```
KAIKKI_LOCAL_DIR=/path/to/kaikki/dumps
DILEMMA_DATA_DIR=/path/to/dilemma/data
```

When `DILEMMA_DATA_DIR` is set and `mg_lookup_scored.json` (or `mg_lookup.json`) is found, the generator will supplement kaikki-derived inflections with Dilemma's more comprehensive mappings. Without it, inflections are extracted from kaikki data only.

#### Kaikki Extraction Date

The build records the Kaikki extraction date (the date Wiktionary was snapshotted, not the date you ran the build) as `extraction_date` in the database's `meta` table. It comes from:

1. For a local dump under `KAIKKI_LOCAL_DIR`: a `.meta` sidecar next to it if there is one, otherwise the file's modification date.
2. For a download: the HTTP `Last-Modified` header, or failing that, the "extracted on YYYY-MM-DD" line on Kaikki's language index page.

On a successful download the downloader writes a tiny `greek_data_<lang>.jsonl.meta` sidecar next to the dump containing `{"extraction_date": ..., "source_url": ..., "downloaded_at": ...}`. Subsequent builds that reuse the cached dump read the sidecar so the extraction date survives across runs. Sidecars are gitignored.

The generator looks for `mg_ranked_forms.json` (pre-ranked inflections) in `data/` in this project, then in `DILEMMA_DATA_DIR`.

#### Lemma Equivalences

Wiktionary and Dilemma sometimes disagree on the canonical lemma for a word (e.g., Wiktionary uses `τρώω` for "eat" while Dilemma files all 165 inflections under `τρώγω`). To bridge this, run:

```bash
cargo run --release --bin generate_mg_equivalences
```

This cross-references the two data sources, uses corpus frequency as a tiebreaker, and writes `data/mg_lemma_equivalences.json`. The dictionary generator loads this automatically. Without it, inflections filed under a different canonical form in Dilemma will be missed.

### Related Projects

- [Dilemma](https://github.com/open-greek/dilemma) - Greek lemmatizer (provides the inflection lookup tables used by Lemma) plus a POS tagger and dependency parser via `dilemma[tagger]`.
- [Greenfield New Testament Greek-English Lexicon](https://github.com/open-greek/greenfield-nt-lexicon) - Public-domain 1896 lexicon with EPUB, Kindle, and StarDict editions.
- [LSJ Greek-English Lexicon](https://github.com/open-greek/lsj-lexicon) - Complete open-licensed Perseus LSJ edition with EPUB, Kindle, StarDict, and reusable JSONL formats.

## Dictionary Content

- **Headwords**: one per distinct Greek spelling, ranked by corpus frequency
- **Entries**: one per part of speech, with gender and key forms ("feminine (plural θάλασσες)") and etymology
- **Senses**: numbered definitions, with usage examples and translations where Wiktionary has them
- **Searchable forms**: inflected forms redirect to their lemma (είπα → λέω), with monotonic and polytonic spellings
- **Inflection tables**: every labelled form, for conjugation and declension tables

### Searchable Forms

Every inflected form of a headword is searchable, ranked by corpus frequency when pre-ranked forms from Dilemma are available, together with its polytonic spellings: forms attested in Greek Wikisource (Dilemma's `mg_polytonic_ranked.json`). Polytonic lookup covers pre-1982 orthography and Katharevousa texts.

A spelling that several headwords share is kept under each of them (λεπτά finds λεπτό and λεπτός). One of them is marked the main owner (`form.is_primary`) so search can rank it first: πήγα lists πηγαίνω before πάω.

### Excluded Content

The following are left out:

- Prefixes and suffixes (e.g., `-ικός`, `προ-`)
- Combining forms and clitics
- Individual letters and symbols
- Abbreviations and contractions

## Troubleshooting

- **Missing data file**: `make sqlite` names the file it couldn't find. Check `LEMMA_DATA`.
- **Download freezes**: download the kaikki dump yourself into `lemma-data`
- **Memory issues**: use the `-l` option to build smaller test dictionaries first

## License

MIT - © 2025-2026 Open Greek

- **Dictionary content and data**: [Creative Commons Attribution-ShareAlike 4.0](https://creativecommons.org/licenses/by-sa/4.0/) (derived from Wiktionary)
- **Frequency data** (`data/el_full.txt`): [MIT License](https://github.com/hermitdave/FrequencyWords/blob/master/LICENSE) (from FrequencyWords/OpenSubtitles)

## Acknowledgments

- Wiktionary contributors for the source data
- [Kaikki.org](https://kaikki.org/) for providing machine-readable Wiktionary dumps
- [Dilemma](https://github.com/open-greek/dilemma) for Greek lemmatization and inflection data
- [FrequencyWords](https://github.com/hermitdave/FrequencyWords) for corpus frequency data (MIT license)
