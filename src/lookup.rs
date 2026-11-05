// Queries against the SQLite database written by `sqlite_export`.
//
// A Greek query is matched by exact spelling, then accent-insensitive
// (`norm`), then by sound (`phonetic`); a Latin query is treated as
// Greeklish and matched by sound only. Results are grouped by headword,
// keeping each headword's best match, and ranked by match quality, then the
// Greeklish reading penalty (x read as χ beats x read as ξ), then corpus
// frequency.

use crate::greek_keys;
use rusqlite::{Connection, params_from_iter};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchKind {
    Exact,
    Norm,
    Phonetic,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub headword_id: i64,
    pub word: String,
    /// The stored spelling that matched: the headword itself or one of its
    /// inflected forms.
    pub matched_form: String,
    pub kind: MatchKind,
    /// Greeklish reading penalty of the key that matched (see
    /// `greek_keys::query_keys`); 0 for Greek queries.
    pub penalty: u8,
    pub frequency: i64,
}

impl Hit {
    fn rank(&self) -> (MatchKind, u8) {
        (self.kind, self.penalty)
    }
}

#[derive(Debug, Clone)]
pub struct EntryView {
    pub pos_display: String,
    pub head: Option<String>,
    pub glosses: Vec<String>,
}

// Within one headword, rows for the headword's own spelling sort first so it
// is preferred as `matched_form` over an inflection with the same key.
const SELECT_FORMS: &str = "
    SELECT f.headword_id, f.form, h.word, h.frequency, f.phonetic
    FROM form f JOIN headword h ON h.id = f.headword_id";
const PREFER_HEADWORD: &str = " ORDER BY f.form = h.word DESC";

pub fn search(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<Hit>> {
    let query = query.trim();
    let mut best: HashMap<i64, Hit> = HashMap::new();

    if greek_keys::is_greek(query) {
        let sql = format!("{} WHERE f.norm = ?1{}", SELECT_FORMS, PREFER_HEADWORD);
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([greek_keys::norm(query)], |r| row_to_hit(r, MatchKind::Norm, 0))?;
        for hit in rows {
            let mut hit = hit?;
            if hit.matched_form == query {
                hit.kind = MatchKind::Exact;
            }
            keep_best(&mut best, hit);
        }
    }

    let keys: HashMap<String, u8> = greek_keys::query_keys(query).into_iter().collect();
    if !keys.is_empty() {
        let placeholders = vec!["?"; keys.len()].join(", ");
        let sql = format!("{} WHERE f.phonetic IN ({}){}", SELECT_FORMS, placeholders, PREFER_HEADWORD);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(keys.keys()), |r| {
            let phonetic: String = r.get(4)?;
            row_to_hit(r, MatchKind::Phonetic, keys.get(&phonetic).copied().unwrap_or(0))
        })?;
        for hit in rows {
            keep_best(&mut best, hit?);
        }
    }

    let mut hits: Vec<Hit> = best.into_values().collect();
    hits.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then(b.frequency.cmp(&a.frequency))
            .then(a.word.cmp(&b.word))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn row_to_hit(r: &rusqlite::Row, kind: MatchKind, penalty: u8) -> rusqlite::Result<Hit> {
    Ok(Hit {
        headword_id: r.get(0)?,
        matched_form: r.get(1)?,
        word: r.get(2)?,
        frequency: r.get(3)?,
        kind,
        penalty,
    })
}

// First row seen wins on ties, which keeps the PREFER_HEADWORD ordering.
fn keep_best(best: &mut HashMap<i64, Hit>, hit: Hit) {
    match best.get(&hit.headword_id) {
        Some(existing) if existing.rank() <= hit.rank() => {}
        _ => {
            best.insert(hit.headword_id, hit);
        }
    }
}

pub fn entries(conn: &Connection, headword_id: i64) -> rusqlite::Result<Vec<EntryView>> {
    let mut entry_stmt = conn.prepare_cached(
        "SELECT id, pos_display, head FROM entry WHERE headword_id = ?1 ORDER BY ord",
    )?;
    let mut sense_stmt = conn.prepare_cached(
        "SELECT gloss FROM sense WHERE entry_id = ?1 ORDER BY ord",
    )?;

    let rows: Vec<(i64, String, Option<String>)> = entry_stmt
        .query_map([headword_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;

    rows.into_iter()
        .map(|(entry_id, pos_display, head)| {
            let glosses = sense_stmt
                .query_map([entry_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(EntryView { pos_display, head, glosses })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite_export::{self, ExportEntry, ExportHeadword, ExportSense};

    fn headword(word: &str, frequency: i64, gloss: &str, forms: &[&str]) -> ExportHeadword {
        ExportHeadword {
            word: word.to_string(),
            frequency,
            entries: vec![ExportEntry {
                pos: "noun".to_string(),
                pos_display: "noun".to_string(),
                head: None,
                etymology: None,
                senses: vec![ExportSense { gloss: gloss.to_string(), example: None, example_translation: None }],
            }],
            forms: forms.iter().map(|f| f.to_string()).collect(),
        }
    }

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        let hws = vec![
            headword("σπίτι", 5000, "house", &["σπίτια", "σπιτιού"]),
            headword("πόλη", 800, "city", &["πόλεις"]),
            headword("πολύ", 9000, "very", &[]),
            headword("λέω", 7000, "to say", &["είπα", "λες"]),
        ];
        sqlite_export::write_to(&mut conn, &hws, &[]).unwrap();
        conn
    }

    fn top(conn: &Connection, q: &str) -> (String, String, MatchKind) {
        let h = search(conn, q, 5).unwrap().into_iter().next().expect("no hit");
        (h.word, h.matched_form, h.kind)
    }

    #[test]
    fn exact_beats_phonetic_even_with_lower_frequency() {
        let conn = db();
        assert_eq!(top(&conn, "πόλη"), ("πόλη".into(), "πόλη".into(), MatchKind::Exact));
    }

    #[test]
    fn accentless_and_misspelled_queries() {
        let conn = db();
        assert_eq!(top(&conn, "σπιτι").2, MatchKind::Norm);
        assert_eq!(top(&conn, "σπητι"), ("σπίτι".into(), "σπίτι".into(), MatchKind::Phonetic));
    }

    #[test]
    fn inflected_form_reports_matched_form() {
        let conn = db();
        assert_eq!(top(&conn, "είπα"), ("λέω".into(), "είπα".into(), MatchKind::Exact));
    }

    #[test]
    fn greeklish_ranks_homophones_by_frequency() {
        let conn = db();
        let words: Vec<String> = search(&conn, "poli", 5).unwrap().into_iter().map(|h| h.word).collect();
        assert_eq!(words, vec!["πολύ", "πόλη"]);
    }

    #[test]
    fn entries_returns_glosses() {
        let conn = db();
        let hit = search(&conn, "spiti", 1).unwrap().remove(0);
        let es = entries(&conn, hit.headword_id).unwrap();
        assert_eq!(es[0].glosses, vec!["house"]);
    }
}
