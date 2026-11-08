// SQLite export of the prepared dictionary.
//
// The `lemma-sqlite` binary's database: an older layout (schema 1) than the
// one `lemma` writes (src/sqlite_export.rs), without that export's limits:
// every inflection and polytonic variant is kept (ranked), every sense is
// kept, and forms claimed by several headwords are kept for each of them
// with an `is_primary` flag instead of being dropped.
//
// Typical lookup (any inflected, polytonic or headword spelling):
//
//   SELECT * FROM lookup WHERE form = 'τρώγαμε' ORDER BY is_primary DESC;
//
// Loose lookup ignoring accents, breathings and case (compute the key in
// your app with the same rules as `form_key()` below: NFD, drop combining
// marks, lowercase, ς -> σ):
//
//   SELECT * FROM lookup WHERE form_key = 'τρωγαμε';
//
// English -> Greek search over definitions:
//
//   SELECT h.word, s.gloss FROM senses_fts f
//   JOIN senses s ON s.id = f.rowid
//   JOIN entries e ON e.id = s.entry_id
//   JOIN headwords h ON h.id = e.headword_id
//   WHERE senses_fts MATCH 'eat';

use crate::entry_processor::Entry;
use crate::dictionary::{
    clean_etymology, greek_word_re, normalize_for_sorting, strip_def_qualifiers,
    strip_transliterations, Dictionary, EntryForms,
};
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// Bumped whenever the table layout changes, so consumers can check
/// `SELECT value FROM meta WHERE key = 'schema_version'`.
pub const SCHEMA_VERSION: &str = "1";

const SCHEMA: &str = r#"
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- One row per dictionary headword. Ids follow alphabetical (sort_key) order.
CREATE TABLE headwords (
    id        INTEGER PRIMARY KEY,
    word      TEXT NOT NULL UNIQUE,
    sort_key  TEXT NOT NULL,      -- lowercased, accent-stripped
    frequency INTEGER NOT NULL    -- OpenSubtitles corpus count, 0 if unattested
);

-- One row per part-of-speech entry of a headword (a headword can have several).
CREATE TABLE entries (
    id          INTEGER PRIMARY KEY,
    headword_id INTEGER NOT NULL REFERENCES headwords(id),
    ord         INTEGER NOT NULL,  -- order within the headword
    pos         TEXT NOT NULL,     -- e.g. noun, verb, adj, name
    head_info   TEXT,              -- e.g. 'feminine (plural θάλασσες)'
    etymology   TEXT
);

-- One row per definition, with its usage example if Wiktionary has one.
CREATE TABLE senses (
    id                  INTEGER PRIMARY KEY,
    entry_id            INTEGER NOT NULL REFERENCES entries(id),
    ord                 INTEGER NOT NULL,  -- order within the entry
    gloss               TEXT NOT NULL,     -- cleaned definition
    raw                 TEXT NOT NULL,     -- definition as extracted, with all qualifiers
    example             TEXT,
    example_translation TEXT,
    example_bold        TEXT               -- JSON [[start,end],...] codepoint offsets to highlight in example
);

-- Greek words in a gloss that are themselves headwords.
CREATE TABLE xrefs (
    sense_id    INTEGER NOT NULL REFERENCES senses(id),
    headword_id INTEGER NOT NULL REFERENCES headwords(id),
    PRIMARY KEY (sense_id, headword_id)
) WITHOUT ROWID;

-- Every spelling that should find a headword: the headword itself, its
-- inflections, and polytonic variants of either.
CREATE TABLE forms (
    form        TEXT NOT NULL,
    form_key    TEXT NOT NULL,     -- lowercased, all diacritics stripped, final ς -> σ
    headword_id INTEGER NOT NULL REFERENCES headwords(id),
    kind        TEXT NOT NULL CHECK (kind IN ('headword', 'inflection', 'polytonic')),
    rank        INTEGER NOT NULL,  -- order within kind for this headword; lower = more frequent
    is_primary  INTEGER NOT NULL,  -- 0 if another headword is the preferred owner of this form
    PRIMARY KEY (form, headword_id)
) WITHOUT ROWID;

-- Wiktionary/Dilemma spelling equivalences (e.g. τρώγω -> τρώω).
CREATE TABLE lemma_equivalences (
    variant   TEXT PRIMARY KEY,
    canonical TEXT NOT NULL
) WITHOUT ROWID;
"#;

const INDEXES: &str = r#"
CREATE INDEX entries_headword ON entries(headword_id);
CREATE INDEX senses_entry     ON senses(entry_id);
CREATE INDEX xrefs_headword   ON xrefs(headword_id);
CREATE INDEX forms_key        ON forms(form_key);
CREATE INDEX forms_headword   ON forms(headword_id);
CREATE INDEX headwords_sort   ON headwords(sort_key);

CREATE VIEW lookup AS
SELECT f.form, f.form_key, f.kind, f.rank, f.is_primary,
       h.id AS headword_id, h.word AS headword
FROM forms f JOIN headwords h ON h.id = f.headword_id;

CREATE VIRTUAL TABLE senses_fts USING fts5(gloss, content='senses', content_rowid='id');
INSERT INTO senses_fts(senses_fts) VALUES ('rebuild');
"#;

pub struct SqliteOptions {
    /// Cap on ranked inflections per headword. None = keep all.
    pub max_inflections: Option<usize>,
    /// Cap on polytonic variants per headword. None = keep all.
    pub max_polytonic: Option<usize>,
    /// Extra key/value rows for the `meta` table.
    pub meta: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub struct SqliteReport {
    pub headwords: usize,
    pub entries: usize,
    pub senses: usize,
    pub forms: usize,
    pub xrefs: usize,
}

/// Accent-, breathing- and case-insensitive key for loose matching:
/// NFD-decompose, drop combining marks (accents, breathings, diaeresis,
/// iota subscript), lowercase, and fold final sigma.
pub fn form_key(s: &str) -> String {
    s.nfd()
        .filter(|c| !('\u{0300}'..='\u{036F}').contains(c))
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c == 'ς' { 'σ' } else { c })
        .collect()
}

/// Write the prepared dictionary to `path`, replacing any existing file.
/// `generator.prepare()` must have been called first.
pub fn write_sqlite(
    generator: &Dictionary,
    equivalences: &HashMap<String, String>,
    path: &Path,
    opts: &SqliteOptions,
) -> Result<SqliteReport, Box<dyn std::error::Error>> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }

    let entries = generator.entries();
    let mut words: Vec<&String> = entries.keys().collect();
    words.sort_by_cached_key(|w| (normalize_for_sorting(w), (*w).clone()));
    let headword_id: HashMap<&str, i64> = words
        .iter()
        .enumerate()
        .map(|(i, w)| (w.as_str(), i as i64 + 1))
        .collect();

    // Form ranking and polytonic expansion are the expensive part; do them in parallel.
    let max_infl = opts.max_inflections.unwrap_or(usize::MAX);
    let max_poly = opts.max_polytonic.unwrap_or(usize::MAX);
    let all_forms: Vec<EntryForms> = words
        .par_iter()
        .map(|w| {
            let es = entries.get(*w).map(|v| v.as_slice()).unwrap_or(&[]);
            generator.entry_forms(w, es, max_infl, max_poly, false)
        })
        .collect();

    let mut conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF; PRAGMA foreign_keys = OFF;",
    )?;
    conn.execute_batch(SCHEMA)?;

    let mut report = SqliteReport::default();
    let tx = conn.transaction()?;
    {
        let mut ins_meta = tx.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2)")?;
        ins_meta.execute(params!["schema_version", SCHEMA_VERSION])?;
        for (k, v) in &opts.meta {
            ins_meta.execute(params![k, v])?;
        }

        let mut ins_head = tx.prepare(
            "INSERT INTO headwords (id, word, sort_key, frequency) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut ins_entry = tx.prepare(
            "INSERT INTO entries (id, headword_id, ord, pos, head_info, etymology) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut ins_sense = tx.prepare(
            "INSERT INTO senses (id, entry_id, ord, gloss, raw, example, example_translation, example_bold) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut ins_xref =
            tx.prepare("INSERT OR IGNORE INTO xrefs (sense_id, headword_id) VALUES (?1, ?2)")?;
        // OR IGNORE: if a spelling appears under two kinds for one headword,
        // the first (headword, then inflection, then polytonic) wins.
        let mut ins_form = tx.prepare(
            "INSERT OR IGNORE INTO forms (form, form_key, headword_id, kind, rank, is_primary) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;

        let mut entry_id: i64 = 0;
        let mut sense_id: i64 = 0;

        for (word, forms) in words.iter().zip(all_forms.iter()) {
            let hid = headword_id[word.as_str()];
            ins_head.execute(params![
                hid,
                word,
                normalize_for_sorting(word),
                generator.frequency_of(word)
            ])?;
            report.headwords += 1;

            let es: &[Entry] = entries.get(*word).map(|v| v.as_slice()).unwrap_or(&[]);
            for (ord, e) in es.iter().enumerate() {
                entry_id += 1;
                let head_info = generator.get_head_info_for_pos(&[e], word);
                let etymology = e
                    .etymology
                    .as_deref()
                    .map(|t| clean_etymology(&strip_transliterations(t.trim())))
                    .filter(|t| !t.is_empty());
                ins_entry.execute(params![entry_id, hid, ord as i64, e.pos, head_info, etymology])?;
                report.entries += 1;

                for (sord, raw) in e.definitions.iter().enumerate() {
                    sense_id += 1;
                    let gloss = strip_def_qualifiers(raw);
                    let ex = e.examples.get(sord).and_then(|x| x.as_ref()).filter(|x| !x.text.is_empty());
                    let bold = ex
                        .and_then(|x| x.bold_text_offsets.as_ref())
                        .map(|o| serde_json::to_string(o).unwrap_or_default());
                    ins_sense.execute(params![
                        sense_id,
                        entry_id,
                        sord as i64,
                        gloss,
                        raw,
                        ex.map(|x| x.text.as_str()),
                        ex.map(|x| x.translation.as_str()).filter(|t| !t.is_empty()),
                        bold
                    ])?;
                    report.senses += 1;

                    let mut linked: HashSet<i64> = HashSet::new();
                    for m in greek_word_re().find_iter(&gloss) {
                        if let Some(&target) = headword_id.get(m.as_str()) {
                            if target != hid && linked.insert(target) {
                                ins_xref.execute(params![sense_id, target])?;
                                report.xrefs += 1;
                            }
                        }
                    }
                }
            }

            // Forms: the headword itself, ranked inflections, then polytonic variants.
            let owned_here =
                |f: &str| generator.iform_owner_of(f).map(|o| o == word.as_str()).unwrap_or(true);
            report.forms += ins_form.execute(params![word, form_key(word), hid, "headword", 0, 1])?;
            for (i, f) in forms.inflections.iter().enumerate() {
                let primary = owned_here(f) as i64;
                report.forms +=
                    ins_form.execute(params![f, form_key(f), hid, "inflection", i as i64 + 1, primary])?;
            }
            for (i, (variant, source)) in forms.polytonic.iter().enumerate() {
                let primary = (source == *word || owned_here(source)) as i64;
                report.forms += ins_form.execute(params![
                    variant,
                    form_key(variant),
                    hid,
                    "polytonic",
                    i as i64 + 1,
                    primary
                ])?;
            }
        }

        let mut ins_eq =
            tx.prepare("INSERT INTO lemma_equivalences (variant, canonical) VALUES (?1, ?2)")?;
        let mut eq: Vec<(&String, &String)> = equivalences.iter().collect();
        eq.sort();
        for (variant, canonical) in eq {
            ins_eq.execute(params![variant, canonical])?;
        }
    }
    tx.commit()?;

    conn.execute_batch(INDEXES)?;
    conn.execute_batch("ANALYZE; VACUUM;")?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::form_key;

    #[test]
    fn form_key_strips_accents_breathings_case_and_final_sigma() {
        assert_eq!(form_key("τρώγαμε"), "τρωγαμε");
        assert_eq!(form_key("ΤΡΏΓΑΜΕ"), "τρωγαμε");
        assert_eq!(form_key("ὁ"), "ο");
        assert_eq!(form_key("τοῦ"), "του");
        assert_eq!(form_key("τῇς"), "τησ");
        assert_eq!(form_key("Λόγος"), "λογοσ");
        assert_eq!(form_key("προϊόν"), "προιον");
    }
}
