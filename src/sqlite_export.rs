// Writes the dictionary as a SQLite database for apps and ad-hoc queries.
//
// The HTML generator stays the source of truth for which forms belong to
// which headword (form-of merging, contested-iform ownership, ranking); it
// hands over plain `ExportHeadword`s and this module only stores them, adding
// the `greek_keys` lookup columns.

use crate::greek_keys;
use rusqlite::{Connection, params};
use std::collections::HashSet;
use std::path::Path;

pub const SCHEMA_VERSION: i64 = 1;

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
    example_translation TEXT
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
";

// Built after the bulk insert, which is faster than maintaining them per row.
const INDEXES: &str = "
CREATE INDEX entry_headword ON entry(headword_id);
CREATE INDEX sense_entry ON sense(entry_id);
CREATE INDEX form_phonetic ON form(phonetic);
CREATE INDEX form_headword ON form(headword_id);
ANALYZE;
";

/// Write a fresh database at `path`, replacing any existing file.
pub fn write_database(
    path: &Path,
    headwords: &[ExportHeadword],
    meta: &[(&str, String)],
) -> rusqlite::Result<ExportStats> {
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    let mut conn = Connection::open(path)?;
    // A failed build is simply rerun, so durability is not needed.
    conn.execute_batch("PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;")?;
    write_to(&mut conn, headwords, meta)
}

pub fn write_to(
    conn: &mut Connection,
    headwords: &[ExportHeadword],
    meta: &[(&str, String)],
) -> rusqlite::Result<ExportStats> {
    conn.execute_batch(SCHEMA)?;
    let mut stats = ExportStats::default();

    let tx = conn.transaction()?;
    {
        let mut ins_meta = tx.prepare("INSERT INTO meta (key, value) VALUES (?1, ?2)")?;
        let mut ins_headword = tx.prepare("INSERT INTO headword (id, word, frequency) VALUES (?1, ?2, ?3)")?;
        let mut ins_entry = tx.prepare(
            "INSERT INTO entry (id, headword_id, ord, pos, pos_display, head, etymology)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;
        let mut ins_sense = tx.prepare(
            "INSERT INTO sense (id, entry_id, ord, gloss, example, example_translation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )?;
        let mut ins_form = tx.prepare(
            "INSERT INTO form (norm, headword_id, form, phonetic) VALUES (?1, ?2, ?3, ?4)",
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
                for (sense_ord, s) in e.senses.iter().enumerate() {
                    stats.senses += 1;
                    ins_sense.execute(params![
                        stats.senses as i64, entry_id, sense_ord as i64,
                        s.gloss, s.example, s.example_translation
                    ])?;
                }
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
    }
    tx.commit()?;
    conn.execute_batch(INDEXES)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                }],
                forms: vec![],
            },
        ]
    }

    fn build() -> (Connection, ExportStats) {
        let mut conn = Connection::open_in_memory().unwrap();
        let stats = write_to(&mut conn, &sample(), &[("source", "test".to_string())]).unwrap();
        (conn, stats)
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
    fn meta_includes_schema_version() {
        let (conn, _) = build();
        let v: String = conn
            .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION.to_string());
    }
}
