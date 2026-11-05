// English lookup terms for English->Greek search.
//
// Terms come from two places: the English glosses of Greek entries
// (`gloss_terms`) and English Wiktionary's translation tables
// (`en_translations`). Both go through `normalize_term`, and so does the
// user's query, so "Houses", "a house" and "house" all meet on "house".

use regex::Regex;
use std::sync::OnceLock;

/// Score for a term that stands alone in a gloss's first clause ("city",
/// "to eat; to chew and swallow food").
pub const SCORE_ONLY_ITEM: u8 = 3;
/// Score for one of several terms in the first clause ("house, home").
pub const SCORE_FIRST_CLAUSE: u8 = 2;
/// Score for a term in a later clause ("...; household").
pub const SCORE_LATER_CLAUSE: u8 = 1;

/// Subtracted (down to SCORE_LATER_CLAUSE) for glosses tagged with one of
/// `DEMOTING_TAGS`, so the everyday word outranks its archaic synonym.
const DEMOTION: u8 = 2;
const DEMOTING_TAGS: &[&str] = &["obsolete", "archaic", "dated", "rare", "dialectal"];

const IRREGULAR_PLURALS: &[(&str, &str)] = &[
    ("children", "child"), ("people", "person"), ("men", "man"), ("women", "woman"),
    ("feet", "foot"), ("teeth", "tooth"), ("mice", "mouse"), ("geese", "goose"),
    ("knives", "knife"), ("wives", "wife"), ("lives", "life"), ("leaves", "leaf"),
];

/// Longest term kept from a gloss; longer items are descriptions, not words.
const MAX_TERM_WORDS: usize = 4;

fn bracketed_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\([^()]*\)|\[[^\[\]]*\]").unwrap())
}

fn is_greek_char(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}')
}

/// Naive English plural stemmer: houses -> house, cities -> city,
/// boxes -> box, plus common irregular plurals. Deliberately leaves -ed/-ing
/// alone (bed, thing).
pub fn stem(word: &str) -> String {
    if let Some((_, singular)) = IRREGULAR_PLURALS.iter().find(|(plural, _)| *plural == word) {
        return singular.to_string();
    }
    let n = word.len();
    if n <= 3 || !word.is_ascii() {
        return word.to_string();
    }
    if word.ends_with("ies") && n > 4 {
        return format!("{}y", &word[..n - 3]);
    }
    if word.ends_with("sses") {
        return word[..n - 2].to_string();
    }
    if word.ends_with("ss") || word.ends_with("us") || word.ends_with("is") {
        return word.to_string();
    }
    for suffix in ["shes", "ches", "xes", "zes"] {
        if word.ends_with(suffix) {
            return word[..n - 2].to_string();
        }
    }
    if word.ends_with('s') {
        return word[..n - 1].to_string();
    }
    word.to_string()
}

/// Lowercase, drop a leading "to"/"a"/"an"/"the", trim punctuation, stem
/// each word. Returns an empty string when nothing word-like remains.
pub fn normalize_term(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut words: Vec<&str> = lower
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '\''))
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() > 1 && matches!(words[0], "to" | "a" | "an" | "the") {
        words.remove(0);
    }
    if !words.iter().any(|w| w.chars().any(|c| c.is_alphabetic())) {
        return String::new();
    }
    words.iter().map(|w| stem(w)).collect::<Vec<_>>().join(" ")
}

/// True for glosses that point at another Greek word ("plural of ελληνικός",
/// "acronym of ...") instead of giving a meaning.
pub fn is_reference_gloss(gloss: &str) -> bool {
    gloss.chars().any(is_greek_char)
}

fn has_demoting_tag(gloss: &str) -> bool {
    bracketed_re().find_iter(gloss).any(|m| {
        m.as_str()
            .split(|c: char| !c.is_alphabetic())
            .any(|w| DEMOTING_TAGS.contains(&w.to_lowercase().as_str()))
    })
}

/// Lookup terms in an English gloss with their scores, highest first.
/// `sense_ord` is the sense's position in its entry; terms from later senses
/// score one less. Reference glosses yield nothing.
pub fn gloss_terms(gloss: &str, sense_ord: usize) -> Vec<(String, u8)> {
    if is_reference_gloss(gloss) {
        return Vec::new();
    }
    let demoted = has_demoting_tag(gloss);

    let mut text = gloss.to_string();
    loop {
        let stripped = bracketed_re().replace_all(&text, "").into_owned();
        if stripped == text { break; }
        text = stripped;
    }

    let clauses: Vec<Vec<&str>> = text
        .split(';')
        .map(|c| c.split(',').map(|i| i.trim()).filter(|i| !i.is_empty()).collect::<Vec<_>>())
        .filter(|c: &Vec<&str>| !c.is_empty())
        .collect();

    let mut terms: Vec<(String, u8)> = Vec::new();
    for (clause_idx, items) in clauses.iter().enumerate() {
        for item in items {
            let term = normalize_term(item);
            if term.is_empty() || term.split(' ').count() > MAX_TERM_WORDS {
                continue;
            }
            let mut score = match (clause_idx, items.len()) {
                (0, 1) => SCORE_ONLY_ITEM,
                (0, _) => SCORE_FIRST_CLAUSE,
                _ => SCORE_LATER_CLAUSE,
            };
            if sense_ord > 0 && score > SCORE_LATER_CLAUSE {
                score -= 1;
            }
            if demoted {
                score = score.saturating_sub(DEMOTION).max(SCORE_LATER_CLAUSE);
            }
            match terms.iter_mut().find(|(t, _)| *t == term) {
                Some(existing) => existing.1 = existing.1.max(score),
                None => terms.push((term, score)),
            }
        }
    }
    terms.sort_by(|a, b| b.1.cmp(&a.1));
    terms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_plurals_only() {
        assert_eq!(stem("houses"), "house");
        assert_eq!(stem("cities"), "city");
        assert_eq!(stem("boxes"), "box");
        assert_eq!(stem("glasses"), "glass");
        assert_eq!(stem("bus"), "bus");
        assert_eq!(stem("thing"), "thing");
        assert_eq!(stem("bed"), "bed");
        assert_eq!(stem("children"), "child");
    }

    #[test]
    fn normalizes_articles_infinitives_and_case() {
        assert_eq!(normalize_term("To Say"), "say");
        assert_eq!(normalize_term("a House"), "house");
        assert_eq!(normalize_term("living rooms."), "living room");
        assert_eq!(normalize_term("to"), "to");
        assert_eq!(normalize_term("..."), "");
    }

    #[test]
    fn lone_first_clause_item_scores_like_whole_gloss() {
        assert_eq!(gloss_terms("to eat; to chew", 0), vec![("eat".to_string(), 3), ("chew".to_string(), 1)]);
    }

    #[test]
    fn archaic_glosses_are_demoted() {
        assert_eq!(gloss_terms("(obsolete) to eat", 0), vec![("eat".to_string(), 1)]);
        assert_eq!(gloss_terms("(colloquial) to eat", 0), vec![("eat".to_string(), 3)]);
    }

    #[test]
    fn scores_whole_gloss_first_clause_and_later_clauses() {
        assert_eq!(gloss_terms("city", 0), vec![("city".to_string(), 3)]);
        assert_eq!(
            gloss_terms("(adverbial, also) house, home (structure built or serving as an abode)", 0),
            vec![("house".to_string(), 2), ("home".to_string(), 2)]
        );
        assert_eq!(
            gloss_terms("to say, tell; to talk", 0),
            vec![("say".to_string(), 2), ("tell".to_string(), 2), ("talk".to_string(), 1)]
        );
    }

    #[test]
    fn later_senses_score_lower() {
        assert_eq!(gloss_terms("brothel", 2), vec![("brothel".to_string(), 2)]);
        assert_eq!(gloss_terms("house; brothel", 1), vec![("house".to_string(), 2), ("brothel".to_string(), 1)]);
    }

    #[test]
    fn skips_references_and_descriptions() {
        assert!(gloss_terms("nominative plural of ελληνικός", 0).is_empty());
        assert!(gloss_terms("a large body of salt water that covers the earth", 0).is_empty());
    }
}
