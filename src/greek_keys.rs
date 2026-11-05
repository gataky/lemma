// Lookup keys for spelling-tolerant Greek search.
//
// `norm` folds case, accents, breathings and final sigma, so σπιτι finds
// σπίτι. `phonetic` goes further and maps a word to how it sounds, merging
// the letters Modern Greek spells differently but pronounces the same
// (ι/η/υ/ει/οι, ο/ω, ε/αι, single/double consonants). The same alphabet is
// produced from Greeklish input via `query_keys`, so "spiti", "σπητι" and
// σπίτι all meet on the key "spiti".
//
// Phonetic alphabet: plain Latin letters, plus "8" for θ (Greeklish
// convention) so θ never collides with τ+χ. Consecutive repeated symbols are
// collapsed.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Upper bound on the Greeklish candidate keys generated for one query.
pub const MAX_QUERY_KEYS: usize = 64;

pub fn is_greek(s: &str) -> bool {
    s.chars().any(is_greek_char)
}

fn is_greek_char(c: char) -> bool {
    matches!(c, '\u{0370}'..='\u{03FF}' | '\u{1F00}'..='\u{1FFF}')
}

/// Lowercase, strip every diacritic, fold final sigma: Σπίτι -> σπιτι.
pub fn norm(s: &str) -> String {
    s.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(|c| c.to_lowercase())
        .map(|c| if c == 'ς' { 'σ' } else { c })
        .collect()
}

struct Letter {
    base: char,
    accent: bool,
    diaeresis: bool,
}

// Decompose into lowercase base letters, remembering the two marks that
// change pronunciation: an accent on the first vowel of a pair (τσάι) or a
// diaeresis on the second (κοροϊδεύω) splits what would otherwise be a
// digraph.
fn greek_letters(s: &str) -> Vec<Letter> {
    let mut out: Vec<Letter> = Vec::new();
    for c in s.nfd() {
        match c {
            '\u{0300}' | '\u{0301}' | '\u{0342}' => {
                if let Some(l) = out.last_mut() { l.accent = true; }
            }
            '\u{0308}' => {
                if let Some(l) = out.last_mut() { l.diaeresis = true; }
            }
            c if is_combining_mark(c) => {}
            c => {
                for lc in c.to_lowercase() {
                    let base = if lc == 'ς' { 'σ' } else { lc };
                    out.push(Letter { base, accent: false, diaeresis: false });
                }
            }
        }
    }
    out
}

fn single_letter(c: char) -> &'static str {
    match c {
        'α' => "a", 'β' => "v", 'γ' => "g", 'δ' => "d", 'ε' => "e", 'ζ' => "z",
        'η' => "i", 'θ' => "8", 'ι' => "i", 'κ' => "k", 'λ' => "l", 'μ' => "m",
        'ν' => "n", 'ξ' => "ks", 'ο' => "o", 'π' => "p", 'ρ' => "r", 'σ' => "s",
        'τ' => "t", 'υ' => "i", 'φ' => "f", 'χ' => "h", 'ψ' => "ps", 'ω' => "o",
        _ => "",
    }
}

/// Sound-based key for a Greek word: σπίτι -> "spiti", θάλασσα -> "8alasa".
pub fn phonetic(s: &str) -> String {
    let letters = greek_letters(s);
    let mut out = String::new();
    let mut i = 0;
    while i < letters.len() {
        let a = letters[i].base;
        if let Some(next) = letters.get(i + 1) {
            let b = next.base;
            if !letters[i].accent && !next.diaeresis {
                let vowel_pair = match (a, b) {
                    ('ο', 'υ') => Some("u"),
                    ('ε', 'ι') | ('ο', 'ι') | ('υ', 'ι') => Some("i"),
                    ('α', 'ι') => Some("e"),
                    _ => None,
                };
                if let Some(m) = vowel_pair {
                    out.push_str(m);
                    i += 2;
                    continue;
                }
                // αυ/ευ are "af"/"ef" before a voiceless consonant or at the
                // end of the word, "av"/"ev" otherwise (αυτός, αύριο).
                if (a == 'α' || a == 'ε') && b == 'υ' {
                    let voiceless = letters
                        .get(i + 2)
                        .map_or(true, |l| "θκξπστφχψ".contains(l.base));
                    out.push(if a == 'α' { 'a' } else { 'e' });
                    out.push(if voiceless { 'f' } else { 'v' });
                    i += 2;
                    continue;
                }
            }
            let consonant_pair = match (a, b) {
                ('μ', 'π') => Some("b"),
                ('ν', 'τ') => Some("d"),
                ('γ', 'κ') | ('γ', 'γ') => Some("g"),
                _ => None,
            };
            if let Some(m) = consonant_pair {
                out.push_str(m);
                i += 2;
                continue;
            }
        }
        out.push_str(single_letter(a));
        i += 1;
    }
    collapse_repeats(&out)
}

// Greeklish spellings and the phonetic symbols each may stand for. Matched
// longest first; ambiguous letters list every reading (h is η or χ, x is χ
// or ξ, u is ου or υ, y is υ or γ).
const GREEKLISH: &[(&str, &[&str])] = &[
    ("th", &["8"]), ("ch", &["h"]), ("ph", &["f"]),
    ("ou", &["u"]), ("ei", &["i"]), ("oi", &["i", "oi"]), ("ui", &["i"]),
    ("ai", &["e", "ai"]),
    ("au", &["af", "av"]), ("ay", &["af", "av"]),
    ("eu", &["ef", "ev"]), ("ey", &["ef", "ev"]),
    ("mp", &["b"]), ("nt", &["d"]), ("gk", &["g"]), ("gg", &["g"]),
    ("a", &["a"]), ("b", &["b", "v"]), ("c", &["k"]), ("d", &["d"]),
    ("e", &["e"]), ("f", &["f"]), ("g", &["g"]), ("h", &["i", "h"]),
    ("i", &["i"]), ("j", &["g"]), ("k", &["k"]), ("l", &["l"]), ("m", &["m"]),
    ("n", &["n"]), ("o", &["o"]), ("p", &["p"]), ("q", &["k"]), ("r", &["r"]),
    ("s", &["s"]), ("t", &["t"]), ("u", &["u", "i"]), ("v", &["v"]),
    ("w", &["o"]), ("x", &["h", "ks"]), ("y", &["i", "g"]), ("z", &["z"]),
    ("8", &["8"]), ("3", &["ks"]),
];

fn greeklish_keys(s: &str) -> Vec<String> {
    let latin: String = s
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(|c| c.to_lowercase())
        .collect();

    let mut candidates: Vec<String> = vec![String::new()];
    let mut rest = latin.as_str();
    while !rest.is_empty() {
        let matched = GREEKLISH.iter().find(|(spelling, _)| rest.starts_with(spelling));
        let Some((spelling, readings)) = matched else {
            // Skip anything that is not Greeklish (spaces, punctuation).
            let skip = rest.chars().next().map_or(1, |c| c.len_utf8());
            rest = &rest[skip..];
            continue;
        };
        rest = &rest[spelling.len()..];
        let mut next: Vec<String> = Vec::new();
        'outer: for c in &candidates {
            for r in *readings {
                if next.len() >= MAX_QUERY_KEYS { break 'outer; }
                next.push(format!("{}{}", c, r));
            }
        }
        candidates = next;
    }

    let mut keys: Vec<String> = Vec::new();
    for c in candidates {
        let k = collapse_repeats(&c);
        if !k.is_empty() && !keys.contains(&k) {
            keys.push(k);
        }
    }
    keys
}

/// Phonetic keys to search for a user query, Greek or Greeklish. Greek input
/// has exactly one key; Greeklish may have several readings.
pub fn query_keys(s: &str) -> Vec<String> {
    if is_greek(s) {
        let k = phonetic(s);
        if k.is_empty() { Vec::new() } else { vec![k] }
    } else {
        greeklish_keys(s)
    }
}

fn collapse_repeats(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last: Option<char> = None;
    for c in s.chars() {
        if Some(c) != last {
            out.push(c);
        }
        last = Some(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_folds_case_accents_and_final_sigma() {
        assert_eq!(norm("Σπίτι"), "σπιτι");
        assert_eq!(norm("λόγος"), "λογοσ");
        assert_eq!(norm("ἄνθρωπος"), "ανθρωποσ");
        assert_eq!(norm("Ϊ"), "ι");
    }

    #[test]
    fn phonetic_merges_homophones() {
        assert_eq!(phonetic("σπίτι"), "spiti");
        assert_eq!(phonetic("σπήτι"), "spiti");
        assert_eq!(phonetic("πόλη"), phonetic("πολύ"));
        assert_eq!(phonetic("καλημέρα"), "kalimera");
        assert_eq!(phonetic("θάλασσα"), "8alasa");
        assert_eq!(phonetic("παιδί"), "pedi");
        assert_eq!(phonetic("ούτε"), "ute");
        assert_eq!(phonetic("γεια"), "gia");
        assert_eq!(phonetic("μπαμπάς"), "babas");
        assert_eq!(phonetic("πάντα"), "pada");
    }

    #[test]
    fn phonetic_respects_accent_and_diaeresis_splits() {
        assert_eq!(phonetic("τσάι"), "tsai");
        assert_eq!(phonetic("κοροϊδεύω"), "koroidevo");
        assert_eq!(phonetic("αϋπνία"), "aipnia");
    }

    #[test]
    fn phonetic_av_ev_voicing() {
        assert_eq!(phonetic("αυτός"), "aftos");
        assert_eq!(phonetic("αύριο"), "avrio");
        assert_eq!(phonetic("ευχαριστώ"), "efharisto");
        assert_eq!(phonetic("Ευρώπη"), "evropi");
    }

    fn reaches(query: &str, word: &str) -> bool {
        query_keys(query).contains(&phonetic(word))
    }

    #[test]
    fn greeklish_reaches_greek() {
        assert!(reaches("spiti", "σπίτι"));
        assert!(reaches("kalhmera", "καλημέρα"));
        assert!(reaches("thalassa", "θάλασσα"));
        assert!(reaches("8alasa", "θάλασσα"));
        assert!(reaches("efharisto", "ευχαριστώ"));
        assert!(reaches("eyxaristw", "ευχαριστώ"));
        assert!(reaches("xeri", "χέρι"));
        assert!(reaches("xerw", "ξέρω"));
        assert!(reaches("geia", "γεια"));
        assert!(reaches("yia", "γεια"));
        assert!(reaches("mpampas", "μπαμπάς"));
        assert!(reaches("tsai", "τσάι"));
        assert!(reaches("ellhnika", "ελληνικά"));
        assert!(reaches("Spíti", "σπίτι"));
    }

    #[test]
    fn greek_query_has_single_key() {
        assert_eq!(query_keys("σπητι"), vec!["spiti".to_string()]);
        assert!(query_keys("").is_empty());
    }

    #[test]
    fn greeklish_expansion_is_capped() {
        assert!(query_keys("hhhhhhhhhhhhxxxxxxxxxxxx").len() <= MAX_QUERY_KEYS);
    }
}
