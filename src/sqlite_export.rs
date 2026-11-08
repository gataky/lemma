// Writes the dictionary as a SQLite database for apps and ad-hoc queries.
//
// The HTML generator stays the source of truth for which forms belong to
// which headword (form-of merging, contested-iform ownership, ranking); it
// hands over plain `ExportHeadword`s and this module only stores them, adding
// the `greek_keys` lookup columns and the English->Greek `en_index`.

use crate::en_translations::EnTranslation;
use crate::english_index;
use crate::greek_keys;
use crate::inflection::InflectionTable;
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 3;

/// Added to a term's gloss score when English Wiktionary's translation table
/// also links the English word to the headword, so curated pairs outrank
/// incidental gloss matches.
pub const TRANSLATION_BOOST: u8 = 3;

pub struct ExportSense {
    pub gloss: String,
    pub example: Option<String>,
    pub example_translation: Option<String>,
}

pub struct ExportEntry {
    pub pos: String,
    pub pos_display: String,
    pub head: Option<String>,
    pub etymology: Option<String>,
    pub senses: Vec<ExportSense>,
    pub tables: Vec<InflectionTable>,
}

pub struct ExportHeadword {
    pub word: String,
    pub frequency: i64,
    pub entries: Vec<ExportEntry>,
    /// Searchable inflected and polytonic forms, excluding the headword.
    pub forms: Vec<String>,
}

#[derive(Debug, Default)]
pub struct ExportStats {
    pub headwords: usize,
    pub entries: usize,
    pub senses: usize,
    pub forms: usize,
    pub inflection_tables: usize,
    pub inflections: usize,
    pub en_terms: usize,
    pub translations_linked: usize,
    /// Translations whose Greek word or phrase matches no headword or form;
    /// they are kept in en_index as text with no headword_id.
    pub translations_unlinked: usize,
}

struct EnRow {
    headword_id: Option<i64>,
    score: u8,
    sense_id: Option<i64>,
    note: Option<String>,
    from_translation: bool,
}

const SCHEMA: &str = "
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE headword (
    id        INTEGER PRIMARY KEY,
    word      TEXT NOT NULL,
    frequency INTEGER NOT NULL
);
-- One row per part of speech of a headword.
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
    -- 1 when the gloss points at another Greek word ('plural of ...')
    -- instead of giving a meaning.
    is_reference        INTEGER NOT NULL
);
-- Every searchable spelling of a headword, the headword itself included,
-- deduplicated per headword by norm (polytonic variants collapse onto their
-- monotonic form).
CREATE TABLE form (
    norm        TEXT NOT NULL,
    headword_id INTEGER NOT NULL REFERENCES headword(id),
    form        TEXT NOT NULL,
    phonetic    TEXT NOT NULL,
    PRIMARY KEY (norm, headword_id)
) WITHOUT ROWID;
-- One inflection table of an entry, or (kind 'head') the forms on its headword
-- line. word is NULL for the headword's own tables; otherwise the table came
-- from a form-of entry merged into this headword, and word is what it inflects.
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
-- The distinct label sets, shared because there are few of them and the
-- database is loaded whole into memory by browser apps.
CREATE TABLE inflection_tags (
    id   INTEGER PRIMARY KEY,
    tags TEXT NOT NULL UNIQUE
);
-- One cell of a table, in Wiktionary's order. Unlike `form` these are not
-- deduplicated and include multi-word forms (θα γράψω).
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
-- English->Greek: normalized English term (english_index::normalize_term)
-- to a Greek word or phrase. headword_id is set when the Greek is one of
-- our headwords (or a form of one), NULL for translation-table words we
-- have no entry for. sense_id is the gloss the term came from; note is the
-- translation table's sense label.
CREATE TABLE en_index (
    term        TEXT NOT NULL,
    greek       TEXT NOT NULL,
    headword_id INTEGER REFERENCES headword(id),
    score       INTEGER NOT NULL,
    sense_id    INTEGER REFERENCES sense(id),
    note        TEXT,
    PRIMARY KEY (term, greek)
) WITHOUT ROWID;
-- Full-text fallback over glosses for English queries en_index misses.
CREATE VIRTUAL TABLE sense_fts USING fts5(
    gloss, content='sense', content_rowid='id', tokenize='porter unicode61'
);
";

// Built after the bulk insert, which is faster than maintaining them per row.
const INDEXES: &str = "
CREATE INDEX entry_headword ON entry(headword_id);
CREATE INDEX sense_entry ON sense(entry_id);
CREATE INDEX form_phonetic ON form(phonetic);
CREATE INDEX form_headword ON form(headword_id);
CREATE INDEX inflection_table_entry ON inflection_table(entry_id);
INSERT INTO sense_fts(sense_fts) VALUES ('rebuild');
ANALYZE;
";

/// Write a fresh database at `path`, replacing any existing file.
pub fn write_database(
    path: &Path,
    headwords: &[ExportHeadword],
    translations: &[EnTranslation],
    meta: &[(&str, String)],
) -> rusqlite::Result<ExportStats> {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let mut conn = Connection::open(path)?;
    // A failed build is simply rerun, so durability is not needed.
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;
    write_to(&mut conn, headwords, translations, meta)
}

pub fn write_to(
    conn: &mut Connection,
    headwords: &[ExportHeadword],
    translations: &[EnTranslation],
    meta: &[(&str, String)],
) -> rusqlite::Result<ExportStats> {
    conn.execute_batch(SCHEMA)?;
    let mut stats = ExportStats::default();
    // Keyed by (term, Greek text); linked rows use the headword's spelling so
    // gloss and translation evidence for the same headword merge.
    let mut en_rows: HashMap<(String, String), EnRow> = HashMap::new();
    // Lowercased headword -> (headword_id, frequency), for linking
    // translation targets. Only exact headword spellings count: inflected
    // forms (λεπτά "money" is also a form of λεπτός "thin") and accent
    // differences (πλαγιά "slope" vs πλάγια "obliquely") pick the wrong word.
    let mut headword_by_spelling: HashMap<String, (i64, i64)> = HashMap::new();
    let mut tags_ids: HashMap<&str, i64> = HashMap::new();

    let tx = conn.transaction()?;
    {
        let mut ins_meta = tx.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2)")?;
        let mut ins_headword = tx.prepare("INSERT INTO headword (id, word, frequency) VALUES (?1, ?2, ?3)")?;
        let mut ins_entry = tx.prepare(
            "INSERT INTO entry (id, headword_id, ord, pos, pos_display, head, etymology)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut ins_sense = tx.prepare(
            "INSERT INTO sense (id, entry_id, ord, gloss, example, example_translation, is_reference)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut ins_form = tx.prepare(
            "INSERT INTO form (norm, headword_id, form, phonetic) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut ins_table = tx.prepare(
            "INSERT INTO inflection_table (id, entry_id, ord, kind, word, label, template, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        let mut ins_tags = tx.prepare("INSERT INTO inflection_tags (id, tags) VALUES (?1, ?2)")?;
        let mut ins_inflection = tx.prepare(
            "INSERT INTO inflection (table_id, ord, form, tags_id) VALUES (?1, ?2, ?3, ?4)",
        )?;

        ins_meta.execute(params!["schema_version", SCHEMA_VERSION.to_string()])?;
        for (k, v) in meta {
            ins_meta.execute(params![k, v])?;
        }

        for hw in headwords {
            stats.headwords += 1;
            let headword_id = stats.headwords as i64;
            ins_headword.execute(params![headword_id, hw.word, hw.frequency])?;

            for (entry_ord, e) in hw.entries.iter().enumerate() {
                stats.entries += 1;
                let entry_id = stats.entries as i64;
                ins_entry.execute(params![
                    entry_id, headword_id, entry_ord as i64,
                    e.pos, e.pos_display, e.head, e.etymology
                ])?;
                for (table_ord, t) in e.tables.iter().enumerate() {
                    stats.inflection_tables += 1;
                    let table_id = stats.inflection_tables as i64;
                    ins_table.execute(params![
                        table_id, entry_id, table_ord as i64,
                        t.kind, t.word, t.label, t.template, t.note
                    ])?;
                    for (form_ord, f) in t.forms.iter().enumerate() {
                        let tags_id = match tags_ids.get(f.tags.as_str()) {
                            Some(&id) => id,
                            None => {
                                let id = tags_ids.len() as i64 + 1;
                                ins_tags.execute(params![id, f.tags])?;
                                tags_ids.insert(&f.tags, id);
                                id
                            }
                        };
                        ins_inflection.execute(params![table_id, form_ord as i64, f.form, tags_id])?;
                        stats.inflections += 1;
                    }
                }
                for (sense_ord, s) in e.senses.iter().enumerate() {
                    stats.senses += 1;
                    let sense_id = stats.senses as i64;
                    ins_sense.execute(params![
                        sense_id, entry_id, sense_ord as i64,
                        s.gloss, s.example, s.example_translation,
                        english_index::is_reference_gloss(&s.gloss)
                    ])?;
                    for (term, score) in english_index::gloss_terms(&s.gloss, sense_ord) {
                        let row = en_rows.entry((term, hw.word.clone())).or_insert(EnRow {
                            headword_id: Some(headword_id),
                            score: 0, sense_id: None, note: None, from_translation: false,
                        });
                        if score > row.score {
                            row.score = score;
                            row.sense_id = Some(sense_id);
                        }
                    }
                }
            }

            let spelling = hw.word.to_lowercase();
            if headword_by_spelling.get(&spelling).is_none_or(|&(_, freq)| hw.frequency > freq) {
                headword_by_spelling.insert(spelling, (headword_id, hw.frequency));
            }

            let mut seen: HashSet<String> = HashSet::new();
            for form in std::iter::once(&hw.word).chain(hw.forms.iter()) {
                let norm = greek_keys::norm(form);
                if norm.is_empty() || !seen.insert(norm.clone()) {
                    continue;
                }
                ins_form.execute(params![norm, headword_id, form, greek_keys::phonetic(form)])?;
                stats.forms += 1;
            }
        }

        for t in translations {
            let term = english_index::normalize_term(&t.english);
            if term.is_empty() {
                continue;
            }
            let headword_id = headword_by_spelling.get(&t.greek.to_lowercase()).map(|&(id, _)| id);
            let greek = match headword_id {
                Some(id) => {
                    stats.translations_linked += 1;
                    headwords[id as usize - 1].word.clone()
                }
                None => {
                    stats.translations_unlinked += 1;
                    t.greek.clone()
                }
            };
            let row = en_rows.entry((term, greek)).or_insert(EnRow {
                headword_id,
                score: 0, sense_id: None, note: None, from_translation: false,
            });
            if !row.from_translation {
                row.from_translation = true;
                row.score += TRANSLATION_BOOST;
                row.note = t.sense.clone();
            }
        }

        let mut ins_en = tx.prepare(
            "INSERT INTO en_index (term, greek, headword_id, score, sense_id, note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        for ((term, greek), row) in &en_rows {
            ins_en.execute(params![term, greek, row.headword_id, row.score, row.sense_id, row.note])?;
        }
        stats.en_terms = en_rows.len();
    }
    tx.commit()?;
    conn.execute_batch(INDEXES)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inflection::InflectedForm;

    fn sample() -> Vec<ExportHeadword> {
        vec![
            ExportHeadword {
                word: "σπίτι".to_string(),
                frequency: 5000,
                entries: vec![ExportEntry {
                    pos: "noun".to_string(),
                    pos_display: "noun".to_string(),
                    head: Some("neuter".to_string()),
                    etymology: None,
                    senses: vec![ExportSense {
                        gloss: "house, home".to_string(),
                        example: Some("Πάω σπίτι.".to_string()),
                        example_translation: Some("I'm going home.".to_string()),
                    }],
                    tables: vec![InflectionTable {
                        kind: "declension".to_string(),
                        word: None,
                        label: None,
                        template: Some("el-nN-ι-ια-2b".to_string()),
                        note: Some("a note".to_string()),
                        forms: vec![
                            InflectedForm { form: "σπίτια".to_string(), tags: "nominative plural".to_string() },
                            InflectedForm { form: "σπιτιού".to_string(), tags: "genitive singular".to_string() },
                            InflectedForm { form: "σπίτια".to_string(), tags: "accusative plural".to_string() },
                            InflectedForm { form: "σπίτια".to_string(), tags: "nominative plural".to_string() },
                        ],
                    }],
                }],
                forms: vec!["σπιτιού".to_string(), "σπίτια".to_string(), "σπιτιοῦ".to_string()],
            },
            ExportHeadword {
                word: "πόλη".to_string(),
                frequency: 800,
                entries: vec![ExportEntry {
                    pos: "noun".to_string(),
                    pos_display: "noun".to_string(),
                    head: None,
                    etymology: None,
                    senses: vec![ExportSense { gloss: "city".to_string(), example: None, example_translation: None }],
                    tables: vec![],
                }],
                forms: vec![],
            },
            ExportHeadword {
                word: "πολύ".to_string(),
                frequency: 9000,
                entries: vec![ExportEntry {
                    pos: "adverb".to_string(),
                    pos_display: "adverb".to_string(),
                    head: None,
                    etymology: None,
                    senses: vec![ExportSense { gloss: "very, much".to_string(), example: None, example_translation: None }],
                    tables: vec![],
                }],
                forms: vec![],
            },
        ]
    }

    fn translations() -> Vec<EnTranslation> {
        let t = |english: &str, greek: &str, sense: &str| EnTranslation {
            english: english.to_string(),
            greek: greek.to_string(),
            sense: Some(sense.to_string()),
        };
        vec![
            t("house", "σπίτι", "abode"),
            t("houses", "σπίτι", "plural, should merge"),
            t("town", "πόλη", "settlement"),
            t("parliament", "βουλή", "not a headword here"),
        ]
    }

    fn build() -> (Connection, ExportStats) {
        let mut conn = Connection::open_in_memory().unwrap();
        let stats = write_to(&mut conn, &sample(), &translations(), &[("source", "test".to_string())]).unwrap();
        (conn, stats)
    }

    fn en_row(conn: &Connection, term: &str, greek: &str) -> Option<(i64, Option<String>)> {
        conn.query_row(
            "SELECT score, note FROM en_index WHERE term = ?1 AND greek = ?2",
            [term, greek],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    }

    #[test]
    fn gloss_terms_and_translations_meet_in_en_index() {
        let (conn, stats) = build();
        // "house, home": first-clause gloss score plus the translation boost.
        assert_eq!(
            en_row(&conn, "house", "σπίτι"),
            Some(((english_index::SCORE_FIRST_CLAUSE + TRANSLATION_BOOST) as i64, Some("abode".to_string())))
        );
        assert_eq!(en_row(&conn, "home", "σπίτι"), Some((english_index::SCORE_FIRST_CLAUSE as i64, None)));
        // Translation-only pair, no gloss mentions "town".
        assert_eq!(en_row(&conn, "town", "πόλη"), Some((TRANSLATION_BOOST as i64, Some("settlement".to_string()))));
        assert_eq!(stats.translations_linked, 3);
        assert_eq!(stats.translations_unlinked, 1);
    }

    #[test]
    fn unlinked_translation_is_kept_as_text() {
        let (conn, _) = build();
        let headword_id: Option<i64> = conn
            .query_row("SELECT headword_id FROM en_index WHERE term = 'parliament' AND greek = 'βουλή'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(headword_id, None);
    }

    #[test]
    fn full_text_fallback_finds_words_inside_glosses() {
        let (conn, _) = build();
        let gloss: String = conn
            .query_row("SELECT s.gloss FROM sense_fts JOIN sense s ON s.id = sense_fts.rowid WHERE sense_fts MATCH 'much'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(gloss, "very, much");
    }

    #[test]
    fn counts_rows_and_dedupes_polytonic_forms() {
        let (_, stats) = build();
        assert_eq!(stats.headwords, 3);
        assert_eq!(stats.entries, 3);
        assert_eq!(stats.senses, 3);
        // σπίτι + σπιτιού + σπίτια (σπιτιοῦ collapses onto σπιτιού), πόλη, πολύ
        assert_eq!(stats.forms, 5);
    }

    #[test]
    fn inflected_form_resolves_to_headword() {
        let (conn, _) = build();
        let word: String = conn
            .query_row(
                "SELECT h.word FROM form f JOIN headword h ON h.id = f.headword_id WHERE f.norm = ?1",
                [greek_keys::norm("ΣΠΙΤΙΟΥ")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(word, "σπίτι");
    }

    #[test]
    fn phonetic_lookup_returns_homophones_by_frequency() {
        let (conn, _) = build();
        let mut stmt = conn
            .prepare(
                "SELECT h.word FROM form f JOIN headword h ON h.id = f.headword_id
                 WHERE f.phonetic = ?1 ORDER BY h.frequency DESC",
            )
            .unwrap();
        let words: Vec<String> = stmt
            .query_map(["poli"], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(words, vec!["πολύ", "πόλη"]);
    }

    #[test]
    fn inflection_cells_are_stored_in_order_with_shared_tags() {
        let (conn, stats) = {
            let mut conn = Connection::open_in_memory().unwrap();
            let stats = write_to(&mut conn, &sample(), &[], &[]).unwrap();
            (conn, stats)
        };
        assert_eq!((stats.inflection_tables, stats.inflections), (1, 4));

        let rows: Vec<(String, String, String, Option<String>)> = conn
            .prepare(
                "SELECT r.form, r.tags, r.template, r.note FROM inflection_row r
                 JOIN entry e ON e.id = r.entry_id JOIN headword h ON h.id = e.headword_id
                 WHERE h.word = 'σπίτι' ORDER BY r.table_ord, r.ord",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(rows.len(), 4, "cells are not deduplicated");
        assert_eq!(rows[1].0, "σπιτιού");
        assert_eq!(rows[1].1, "genitive singular");
        assert_eq!(rows[0].2, "el-nN-ι-ια-2b");
        assert_eq!(rows[0].3.as_deref(), Some("a note"));

        let distinct: i64 =
            conn.query_row("SELECT COUNT(*) FROM inflection_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(distinct, 3);
    }

    #[test]
    fn meta_includes_schema_version() {
        let (conn, _) = build();
        let v: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }
}
