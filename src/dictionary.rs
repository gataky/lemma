// Turns the parsed entries into the final dictionary: folds form-of entries
// into their parents, decides which headword owns a spelling several claim,
// ranks each headword's searchable forms, and cleans glosses, head lines and
// etymologies for export.

use crate::dilemma::DilemmaInflections;
use crate::entry_processor::{Entry, EntryMap};
use crate::frequency::FrequencyRanker;
use crate::inflection::{merge_tables, InflectionTable};
use crate::sqlite_export::{ExportEntry, ExportHeadword, ExportSense};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;

pub const MAX_INFLECTIONS: usize = 255;
pub const MAX_POLYTONIC: usize = 255;

fn pos_map() -> &'static HashMap<&'static str, &'static str> {
    static M: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert("noun", "ουσ.");
        m.insert("verb", "ρ.");
        m.insert("participle", "μτχ.");
        m.insert("adj", "επίθ.");
        m.insert("adjective", "επίθ.");
        m.insert("adv", "επίρρ.");
        m.insert("adverb", "επίρρ.");
        m.insert("num", "αριθμ.");
        m.insert("numeral", "αριθμ.");
        m.insert("name", "κύρ.όν.");
        m.insert("proper noun", "κύρ.όν.");
        m.insert("article", "άρθρ.");
        m
    })
}

fn strip_tags() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        let mut s = HashSet::new();
        s.insert("masculine");
        s.insert("feminine");
        s.insert("neuter");
        s.insert("participle");
        s.insert("singular");
        s.insert("plural");
        s
    })
}

// Breathing maps: monotonic -> (smooth, rough)
fn breathing() -> &'static HashMap<char, (char, char)> {
    static M: OnceLock<HashMap<char, (char, char)>> = OnceLock::new();
    M.get_or_init(|| {
        let data: &[(char, (char, char))] = &[
            ('α', ('ἀ', 'ἁ')), ('ά', ('ἄ', 'ἅ')),
            ('ε', ('ἐ', 'ἑ')), ('έ', ('ἔ', 'ἕ')),
            ('η', ('ἠ', 'ἡ')), ('ή', ('ἤ', 'ἥ')),
            ('ι', ('ἰ', 'ἱ')), ('ί', ('ἴ', 'ἵ')),
            ('ο', ('ὀ', 'ὁ')), ('ό', ('ὄ', 'ὅ')),
            ('υ', ('ὐ', 'ὑ')), ('ύ', ('ὔ', 'ὕ')),
            ('ω', ('ὠ', 'ὡ')), ('ώ', ('ὤ', 'ὥ')),
            ('Α', ('Ἀ', 'Ἁ')), ('Ά', ('Ἄ', 'Ἅ')),
            ('Ε', ('Ἐ', 'Ἑ')), ('Έ', ('Ἔ', 'Ἕ')),
            ('Η', ('Ἠ', 'Ἡ')), ('Ή', ('Ἤ', 'Ἥ')),
            ('Ι', ('Ἰ', 'Ἱ')), ('Ί', ('Ἴ', 'Ἵ')),
            ('Ο', ('Ὀ', 'Ὁ')), ('Ό', ('Ὄ', 'Ὅ')),
            ('Υ', ('Ὑ', 'Ὑ')), ('Ύ', ('Ὕ', 'Ὕ')),
            ('Ω', ('Ὠ', 'Ὡ')), ('Ώ', ('Ὤ', 'Ὥ')),
        ];
        data.iter().cloned().collect()
    })
}

fn acute_to_grave() -> &'static HashMap<char, char> {
    static M: OnceLock<HashMap<char, char>> = OnceLock::new();
    M.get_or_init(|| {
        let data: &[(char, char)] = &[
            ('ά', 'ὰ'), ('έ', 'ὲ'), ('ή', 'ὴ'), ('ί', 'ὶ'), ('ό', 'ὸ'), ('ύ', 'ὺ'), ('ώ', 'ὼ'),
            ('Ά', 'Ὰ'), ('Έ', 'Ὲ'), ('Ή', 'Ὴ'), ('Ί', 'Ὶ'), ('Ό', 'Ὸ'), ('Ύ', 'Ὺ'), ('Ώ', 'Ὼ'),
        ];
        data.iter().cloned().collect()
    })
}

fn acute_to_circumflex() -> &'static HashMap<char, char> {
    static M: OnceLock<HashMap<char, char>> = OnceLock::new();
    M.get_or_init(|| {
        let data: &[(char, char)] = &[('ά', 'ᾶ'), ('ή', 'ῆ'), ('ί', 'ῖ'), ('ύ', 'ῦ'), ('ώ', 'ῶ')];
        data.iter().cloned().collect()
    })
}

const DIPHTHONG_FIRSTS: &str = "αεοΑΕΟ";
const DIPHTHONG_SECONDS: &str = "ιυίύΙΥΊΎ";

// --- Public API types ---

pub struct BuildParams {
    pub source_lang: String,
    pub limit_percent: Option<f64>,
    pub max_inflections: Option<usize>,
}

/// Searchable forms of one headword, as computed by `Dictionary::entry_forms`.
pub(crate) struct EntryForms {
    /// Ranked inflected forms, most frequent first.
    pub inflections: Vec<String>,
    /// (polytonic variant, monotonic form it was derived from).
    pub polytonic: Vec<(String, String)>,
}

pub struct Dictionary<'a> {
    pub entries: EntryMap,
    pub output_dir: PathBuf,
    pub params: BuildParams,
    dilemma: Option<&'a DilemmaInflections>,
    frequency: FrequencyRanker,
    use_ranked_forms: bool,
    iform_owner: HashMap<String, String>,
}

impl<'a> Dictionary<'a> {
    pub fn new(entries: EntryMap, params: BuildParams, dilemma: Option<&'a DilemmaInflections>) -> Self {
        let use_ranked_forms = dilemma.as_ref().map(|d| d.has_ranked_forms()).unwrap_or(false);
        if use_ranked_forms {
            println!("Using pre-ranked forms from dilemma for inflection ordering");
        }
        let freq = FrequencyRanker::new();

        // Build directory name has no date and no version - lemma's stable
        // naming convention. The limit-percent suffix stays because it
        // identifies a test build vs. the real one.
        let mut output_dir = format!("lemma_greek_{}", params.source_lang);
        if let Some(p) = params.limit_percent {
            output_dir = format!("{}_{}pct", output_dir, p);
        }

        Self {
            entries,
            output_dir: PathBuf::from(output_dir),
            params,
            dilemma,
            frequency: freq,
            use_ranked_forms,
            iform_owner: HashMap::new(),
        }
    }

    /// Fold form-of entries into their parents and decide which headword owns
    /// each contested inflected form. Must run before anything is exported.
    pub fn prepare(&mut self) {
        self.merge_form_of_into_parents();
        self.build_iform_owners();
    }

    fn merge_form_of_into_parents(&mut self) {
        let mut merged: Vec<String> = Vec::new();
        let mut parent_of: HashMap<String, String> = HashMap::new();

        // Collect candidates
        let candidates: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, es)| es.iter().all(|e| !e.form_of_targets.is_empty()))
            .map(|(k, _)| k.clone())
            .collect();

        for word in candidates {
            let targets: Vec<String> = {
                let entries = self.entries.get(&word).unwrap();
                let mut seen = HashSet::new();
                let mut ts = Vec::new();
                for e in entries {
                    for t in &e.form_of_targets {
                        if !seen.contains(t) && self.entries.contains_key(t) {
                            ts.push(t.clone());
                            seen.insert(t.clone());
                        }
                    }
                }
                ts
            };

            if targets.is_empty() { continue; }

            let best = if targets.len() == 1 {
                targets[0].clone()
            } else {
                // Match Python's `max(targets, key=...)` tiebreak: first element wins.
                let freq = &self.frequency;
                let mut best = targets[0].clone();
                let mut best_f = freq.frequency(&best);
                for t in targets.iter().skip(1) {
                    let f = freq.frequency(t);
                    if f > best_f {
                        best = t.clone();
                        best_f = f;
                    }
                }
                best
            };

            parent_of.insert(word.clone(), best);
            merged.push(word);
        }

        // Words that are forms of each other (αλφάβητο <-> αλφάβητος) have no
        // true parent. Merging them would delete them all, so they stay.
        let cyclic = words_on_cycles(&parent_of);
        parent_of.retain(|word, _| !cyclic.contains(word));
        merged.retain(|word| !cyclic.contains(word));

        for word in &merged {
            if let Some(parent_entries) = self.entries.get_mut(root_of(word, &parent_of)) {
                for pe in parent_entries.iter_mut() {
                    if !pe.inflections.contains(word) {
                        pe.inflections.push(word.clone());
                    }
                }
            }
        }

        self.move_tables_to_parents(&merged, &parent_of);

        let merged_count = merged.len();
        let set: HashSet<String> = merged.into_iter().collect();
        self.entries.remove_many(&set);
        println!("  Merged {} form-of entries into parent headwords", merged_count);
        if !cyclic.is_empty() {
            println!("  Kept {} headwords that are forms of each other", cyclic.len());
        }
    }

    fn build_iform_owners(&mut self) {
        let mut all_claims: HashMap<String, Vec<String>> = HashMap::new();

        for (word, entries) in self.entries.iter() {
            for e in entries {
                for inf in &e.inflections {
                    let claims = all_claims.entry(inf.clone()).or_default();
                    if !claims.contains(word) {
                        claims.push(word.clone());
                    }
                }
            }
            if self.use_ranked_forms {
                if let Some(dilemma) = self.dilemma {
                    if let Some(ranked) = dilemma.get_ranked_forms(word) {
                        for inf in ranked {
                            let claims = all_claims.entry(inf.clone()).or_default();
                            if !claims.contains(word) {
                                claims.push(word.clone());
                            }
                        }
                    }
                }
            }
        }

        let equiv_canonical: HashMap<String, String> = self
            .dilemma
            .map(|d| d.equivalences.clone())
            .unwrap_or_default();

        let mut iform_owner: HashMap<String, String> = HashMap::new();
        let mut contested = 0;
        for (iform, headwords) in &all_claims {
            if headwords.len() <= 1 { continue; }

            // First preference: if one contestant is a variant of another
            // per dilemma's equivalences map, the canonical wins.
            let mut winner: Option<String> = None;
            for hw in headwords {
                if let Some(canonical) = equiv_canonical.get(hw) {
                    if headwords.contains(canonical) {
                        winner = Some(canonical.clone());
                        break;
                    }
                }
            }

            // Otherwise pick the highest-frequency headword. Alphabetical
            // stable tiebreak keeps output deterministic when frequencies
            // match (including the all-zero case).
            if winner.is_none() {
                let mut best: Option<(&String, i64)> = None;
                for hw in headwords {
                    let f = self.frequency.frequency(hw);
                    match &best {
                        None => best = Some((hw, f)),
                        Some((bw, bf)) => {
                            if f > *bf || (f == *bf && hw < *bw) {
                                best = Some((hw, f));
                            }
                        }
                    }
                }
                winner = best.map(|(w, _)| w.clone());
            }

            let Some(winner) = winner else { continue; };
            contested += 1;
            for hw in headwords {
                if hw != &winner {
                    iform_owner.insert(iform.clone(), winner.clone());
                }
            }
        }
        println!("  Deduplicated {} contested iforms (canonical form + highest-frequency tiebreak)", contested);
        self.iform_owner = iform_owner;
    }

    /// Compute the full ordered set of searchable inflected forms for a
    /// headword: ranked single-word inflections (capped at max_inflections),
    /// de-duplicated against iforms owned by a higher-frequency headword, then
    /// extended with polytonic variants. This is exactly the set emitted as
    /// `<idx:iform>` by the idx path and as `<match>` entries by the EPUB3 SKM,
    /// keeping the two outputs in agreement.
    fn entry_variations(&self, word: &str, entries: &[Entry]) -> Vec<String> {
        let max_inflections = self.params.max_inflections.unwrap_or(MAX_INFLECTIONS);
        let forms = self.entry_forms(word, entries, max_inflections, MAX_POLYTONIC, true);
        let mut all_variations = forms.inflections;
        all_variations.extend(forms.polytonic.into_iter().map(|(variant, _source)| variant));
        all_variations
    }

    /// The searchable forms of a headword, split by kind. Inflections come
    /// back ranked (most frequent first) and capped at `max_inflections`;
    /// polytonic variants are paired with the monotonic form they were
    /// derived from (the headword itself or one of the inflections) and
    /// capped at `max_polytonic`.
    ///
    /// When `drop_contested` is true, inflections owned by another headword
    /// (see `build_iform_owners`) are removed before polytonic expansion, so
    /// each spelling belongs to one headword. The `form` table does this, a
    /// rule inherited from the Kindle build.
    pub(crate) fn entry_forms(
        &self,
        word: &str,
        entries: &[Entry],
        max_inflections: usize,
        max_polytonic: usize,
        drop_contested: bool,
    ) -> EntryForms {
        // Combine all inflections from all entries
        let mut all_inflections: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for e in entries {
            for inf in &e.inflections {
                if seen.insert(inf.clone()) {
                    all_inflections.push(inf.clone());
                }
            }
        }

        let single_word: Vec<String> = all_inflections.into_iter().filter(|i| !i.contains(' ')).collect();

        let is_proper_noun = entries.iter().any(|e| {
            let lower = e.pos.to_lowercase();
            lower.contains("proper") || lower.contains("name")
        });

        let mut inflections: Vec<String> = if is_proper_noun {
            single_word.into_iter().take(max_inflections).collect()
        } else if self.use_ranked_forms {
            self.select_ranked_inflections(word, &single_word, max_inflections)
        } else {
            let ranked = self.rank_inflections(&single_word);
            ranked.into_iter().take(max_inflections).collect()
        };

        // Remove iforms owned by a higher-frequency headword
        if drop_contested {
            inflections.retain(|inf| {
                self.iform_owner.get(inf).map(|w| w.as_str() == word).unwrap_or(true)
            });
        }

        // Polytonic expansion (always enabled in the unified edition).
        let mut all_forms: Vec<String> = Vec::with_capacity(inflections.len() + 1);
        all_forms.push(word.to_string());
        all_forms.extend(inflections.iter().cloned());
        let mut polytonic: Vec<(String, String)> = Vec::new();
        let mut seen_poly: HashSet<String> = all_forms.iter().cloned().collect();

        let dilemma_has_poly = self.dilemma.map(|d| d.has_polytonic_ranked()).unwrap_or(false);
        if dilemma_has_poly {
            let dilemma = self.dilemma.unwrap();
            for form in &all_forms {
                for pv in dilemma.get_polytonic_variants(form) {
                    if seen_poly.insert(pv.clone()) {
                        polytonic.push((pv.clone(), form.clone()));
                    }
                }
            }
        } else {
            for form in &all_forms {
                for pv in polytonic_variants(form) {
                    if seen_poly.insert(pv.clone()) {
                        polytonic.push((pv, form.clone()));
                    }
                }
            }
        }
        polytonic.truncate(max_polytonic);

        EntryForms { inflections, polytonic }
    }

    /// Every headword in dictionary sort order, ready for the SQLite export.
    /// Reflects form-of merging and contested-iform ownership, so it must be
    /// called after `prepare`.
    pub fn export_headwords(&self) -> Vec<ExportHeadword> {
        let mut sorted_keys: Vec<&String> = self.entries.keys().collect();
        sorted_keys.sort_by_cached_key(|k| normalize_for_sorting(k));

        sorted_keys
            .into_iter()
            .map(|word| {
                let entries = self.entries.get(word).unwrap();
                ExportHeadword {
                    word: word.clone(),
                    frequency: self.frequency.frequency(word),
                    entries: entries.iter().map(|e| self.export_entry(word, e)).collect(),
                    forms: self.entry_variations(word, entries),
                }
            })
            .collect()
    }

    fn export_entry(&self, word: &str, entry: &Entry) -> ExportEntry {
        let mut effective_pos = entry.pos.clone();
        if entry.pos == "verb" && !entry.definitions.is_empty()
            && entry.definitions.iter().all(|d| def_has_tag(d, "participle"))
        {
            effective_pos = "participle".to_string();
        }

        let senses = entry
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.as_str() != "No definition available")
            .map(|(i, d)| {
                let ex = entry.examples.get(i).and_then(|e| e.as_ref()).filter(|ex| !ex.text.is_empty());
                ExportSense {
                    gloss: strip_def_qualifiers(d),
                    example: ex.map(|ex| ex.text.clone()),
                    example_translation: ex.map(|ex| ex.translation.clone()).filter(|t| !t.is_empty()),
                }
            })
            .collect();

        let etymology = entry
            .etymology
            .as_deref()
            .map(|t| clean_etymology(&strip_transliterations(t.trim())))
            .filter(|t| !t.is_empty());

        ExportEntry {
            pos: entry.pos.clone(),
            pos_display: self.format_pos(&effective_pos),
            head: self.get_head_info_for_pos(&[entry], word),
            etymology,
            senses,
            tables: entry.tables.clone(),
        }
    }

    // A merged form-of entry can still have a table of its own (χειρότερος
    // under κακός, καφεδάκι under καφές). It survives on the parent, labelled
    // with the word it inflects.
    fn move_tables_to_parents(&mut self, merged: &[String], parent_of: &HashMap<String, String>) {
        for word in merged {
            let root = root_of(word, parent_of);
            let children: Vec<(String, Vec<InflectionTable>)> = self
                .entries
                .get_mut(word)
                .map(|es| es.iter_mut().map(|e| (e.pos.clone(), std::mem::take(&mut e.tables))).collect())
                .unwrap_or_default();
            let parent_entries = self.entries.get_mut(root).unwrap();
            for (pos, mut tables) in children {
                for table in &mut tables {
                    table.word.get_or_insert_with(|| word.clone());
                }
                let target = parent_entries.iter().position(|e| e.pos == pos).unwrap_or(0);
                merge_tables(&mut parent_entries[target].tables, tables);
            }
        }
    }

    fn rank_inflections(&self, forms: &[String]) -> Vec<String> {
        let mut tier1: Vec<(String, i64)> = Vec::new();
        let mut tier2: Vec<(String, i64)> = Vec::new();
        let mut tier3: Vec<(String, i64)> = Vec::new();

        for form in forms {
            let f = self.frequency.frequency(form);
            let c = self.dilemma.map(|d| d.confidence_for(form)).unwrap_or(0);
            if f > 0 {
                tier1.push((form.clone(), f));
            } else if c >= 3 {
                tier2.push((form.clone(), c));
            } else {
                tier3.push((form.clone(), c));
            }
        }

        tier1.sort_by(|a, b| b.1.cmp(&a.1));
        tier2.sort_by(|a, b| b.1.cmp(&a.1));
        tier3.sort_by(|a, b| b.1.cmp(&a.1));

        let mut result = Vec::with_capacity(forms.len());
        for (f, _) in tier1 { result.push(f); }
        for (f, _) in tier2 { result.push(f); }
        for (f, _) in tier3 { result.push(f); }
        result
    }

    fn select_ranked_inflections(&self, headword: &str, entry_inflections: &[String], max_count: usize) -> Vec<String> {
        if entry_inflections.is_empty() { return Vec::new(); }
        let Some(dilemma) = self.dilemma else { return entry_inflections.iter().take(max_count).cloned().collect(); };
        let Some(ranked) = dilemma.get_ranked_forms(headword) else {
            return entry_inflections.iter().take(max_count).cloned().collect();
        };

        let ranked_lower: HashSet<String> = ranked.iter().map(|f| crate::entry_processor::py_lower(f)).collect();

        let mut result: Vec<String> = Vec::new();
        let mut result_lower: HashSet<String> = HashSet::new();

        for f in entry_inflections {
            let low = crate::entry_processor::py_lower(f);
            if !ranked_lower.contains(&low) && !result_lower.contains(&low) {
                result.push(f.clone());
                result_lower.insert(low);
            }
        }
        for form in ranked {
            let low = crate::entry_processor::py_lower(form);
            if !result_lower.contains(&low) {
                result.push(form.clone());
                result_lower.insert(low);
            }
            if result.len() >= max_count { break; }
        }
        result.truncate(max_count);
        result
    }

    fn format_pos(&self, pos: &str) -> String {
        let pos_display = if pos.is_empty() { "unknown" } else { pos };
        if self.params.source_lang == "el" {
            if let Some(m) = pos_map().get(pos_display.to_lowercase().as_str()) {
                return m.to_string();
            }
        }
        pos_display.to_string()
    }

    pub(crate) fn get_head_info_for_pos(&self, pos_entries: &[&Entry], word: &str) -> Option<String> {
        for e in pos_entries {
            if let Some(head_exp) = &e.head_expansion {
                let stripped = strip_head_expansion(head_exp, word);
                if !stripped.is_empty() {
                    if let Some(r) = format_head_for_pos(&stripped) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }

}

// --- Free helpers ---

pub(crate) fn normalize_for_sorting(word: &str) -> String {
    // lowercase (Greek final-sigma aware), strip accents, remove non Greek/Latin/digit
    let lower = crate::entry_processor::py_lower(word);
    let stripped: String = lower.chars().map(|c| match c {
        'ά' => 'α', 'έ' => 'ε', 'ή' => 'η', 'ί' => 'ι', 'ό' => 'ο', 'ύ' => 'υ', 'ώ' => 'ω',
        'ΐ' => 'ι', 'ΰ' => 'υ', 'ϊ' => 'ι', 'ϋ' => 'υ',
        'Ά' => 'α', 'Έ' => 'ε', 'Ή' => 'η', 'Ί' => 'ι', 'Ό' => 'ο', 'Ύ' => 'υ', 'Ώ' => 'ω',
        _ => c,
    }).collect();
    re_non_sort_char().replace_all(&stripped, "").into_owned()
}

fn re_paren_prefix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\(([^)]+)\)").unwrap())
}
fn re_paren_prefix_ws() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\(([^)]+)\)\s*").unwrap())
}
fn re_strip_translit_a() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\s*\([^)]*[\u00C0-\u024F\u1E00-\u1EFF][^)]*\)").unwrap())
}
fn re_strip_translit_b() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"\s*\([^)"]*"[^)]*\)"#).unwrap())
}
fn re_head_prefix() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^\([A-Za-z\u00C0-\u024F\s]+\)\s*").unwrap())
}
fn re_head_format() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^([mfn](?:\s+or\s+[mfn])*)(\s+(?:pl|sg))?(\s+\(.*\))?\s*$").unwrap())
}
fn re_non_sort_char() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[^\u0370-\u03FF\u1F00-\u1FFFA-Za-z0-9]").unwrap())
}
fn gender_word_re(ab: &str) -> &'static Regex {
    static M: OnceLock<Regex> = OnceLock::new();
    static F: OnceLock<Regex> = OnceLock::new();
    static N: OnceLock<Regex> = OnceLock::new();
    match ab {
        "m" => M.get_or_init(|| Regex::new(r"\bm\b").unwrap()),
        "f" => F.get_or_init(|| Regex::new(r"\bf\b").unwrap()),
        _ => N.get_or_init(|| Regex::new(r"\bn\b").unwrap()),
    }
}

pub(crate) fn def_has_tag(definition: &str, tag: &str) -> bool {
    match re_paren_prefix().captures(definition) {
        Some(c) => {
            let group = c.get(1).unwrap().as_str();
            group.split(',').any(|t| t.trim().to_lowercase() == tag)
        }
        None => false,
    }
}

pub(crate) fn strip_def_qualifiers(definition: &str) -> String {
    let stripped = strip_transliterations(definition);
    match re_paren_prefix_ws().captures(&stripped) {
        None => stripped,
        Some(c) => {
            let group = c.get(1).unwrap().as_str();
            let tags: Vec<&str> = group.split(',').map(|t| t.trim()).collect();
            let remaining: Vec<&&str> = tags.iter().filter(|t| !strip_tags().contains(t.to_lowercase().as_str())).collect();
            let end = c.get(0).unwrap().end();
            let rest = &stripped[end..];
            if !remaining.is_empty() {
                let joined: Vec<&str> = remaining.iter().map(|s| **s).collect();
                format!("({}) {}", joined.join(", "), rest)
            } else {
                rest.to_string()
            }
        }
    }
}

pub(crate) fn strip_transliterations(text: &str) -> String {
    let t1 = re_strip_translit_a().replace_all(text, "").into_owned();
    re_strip_translit_b().replace_all(&t1, "").into_owned()
}

pub(crate) fn clean_etymology(text: &str) -> String {
    let mut out = text.to_string();
    for marker in &["Typological comparisons", "See also", "\n*", "\n•"] {
        if let Some(pos) = out.find(marker) {
            if pos > 0 {
                out.truncate(pos);
            }
        }
    }
    out.trim().to_string()
}

fn strip_head_expansion(head_exp: &str, word: &str) -> String {
    let mut text = head_exp.to_string();
    if let Some(bullet_pos) = text.find('•') {
        text = text[bullet_pos + '•'.len_utf8()..].trim().to_string();
    } else if text.starts_with(word) {
        text = text[word.len()..].trim().to_string();
    }
    re_head_prefix().replace(&text, "").trim().to_string()
}

fn format_head_for_pos(stripped: &str) -> Option<String> {
    if stripped.is_empty() { return None; }
    let caps = re_head_format().captures(stripped)?;
    let mut genders = caps.get(1)?.as_str().to_string();
    let number = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
    let parens = caps.get(3).map(|m| m.as_str().trim()).unwrap_or("");

    // Substitute m/f/n gender codes with their full names, as word-boundary
    // matches so "or" between them isn't touched.
    for (ab, full) in &[("m", "masculine"), ("f", "feminine"), ("n", "neuter")] {
        let re = gender_word_re(ab);
        genders = re.replace_all(&genders, *full).into_owned();
    }
    let mut parts = vec![genders];
    if !number.is_empty() {
        parts.push(if number == "pl" { "plural".to_string() } else { "singular".to_string() });
    }
    let mut result = parts.join(" ");
    if !parens.is_empty() {
        result.push(' ');
        result.push_str(parens);
    }
    Some(result)
}

// --- Polytonic variant generation ---

fn breathing_variants(form: &str) -> Vec<String> {
    if form.is_empty() { return Vec::new(); }
    let chars: Vec<char> = form.chars().collect();
    if !breathing().contains_key(&chars[0]) { return Vec::new(); }

    if chars.len() >= 2 && DIPHTHONG_FIRSTS.contains(chars[0]) && DIPHTHONG_SECONDS.contains(chars[1]) {
        let Some((smooth, rough)) = breathing().get(&chars[1]).copied() else { return Vec::new(); };
        let rest: String = chars[2..].iter().collect();
        return vec![
            format!("{}{}{}", chars[0], smooth, rest),
            format!("{}{}{}", chars[0], rough, rest),
        ];
    }
    let Some((smooth, rough)) = breathing().get(&chars[0]).copied() else { return Vec::new(); };
    let rest: String = chars[1..].iter().collect();
    vec![format!("{}{}", smooth, rest), format!("{}{}", rough, rest)]
}

fn accent_variants(form: &str) -> Vec<String> {
    let mut results = Vec::new();
    let chars: Vec<char> = form.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if acute_to_grave().contains_key(c) {
            if let Some(grave) = acute_to_grave().get(c).copied() {
                let mut new_chars = chars.clone();
                new_chars[i] = grave;
                results.push(new_chars.into_iter().collect());
            }
            if let Some(circ) = acute_to_circumflex().get(c).copied() {
                let mut new_chars = chars.clone();
                new_chars[i] = circ;
                results.push(new_chars.into_iter().collect());
            }
            break;
        }
    }
    results
}

fn polytonic_variants(form: &str) -> Vec<String> {
    let mut results: HashSet<String> = HashSet::new();
    for bv in breathing_variants(form) {
        results.insert(bv);
    }
    for av in accent_variants(form) {
        for bv in breathing_variants(&av) {
            results.insert(bv);
        }
        results.insert(av);
    }
    results.remove(form);
    results.into_iter().collect()
}

/// Words whose chain of form-of parents leads back to themselves.
fn words_on_cycles(parent_of: &HashMap<String, String>) -> HashSet<String> {
    parent_of
        .keys()
        .filter(|start| {
            let mut current = parent_of.get(*start);
            for _ in 0..parent_of.len() {
                match current {
                    None => return false,
                    Some(word) if word == *start => return true,
                    Some(word) => current = parent_of.get(word),
                }
            }
            false
        })
        .cloned()
        .collect()
}

/// The end of a word's chain of form-of parents, since a parent can itself have
/// been merged (Γιωργάκης -> Γιώργος -> Γεώργιος). `parent_of` must have no
/// cycles.
fn root_of<'a>(word: &'a String, parent_of: &'a HashMap<String, String>) -> &'a String {
    let mut root = word;
    while let Some(parent) = parent_of.get(root) {
        root = parent;
    }
    root
}

#[cfg(test)]
mod form_of_tests {
    use super::*;

    fn links(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn finds_mutual_and_self_links_but_not_words_leading_into_them() {
        let parent_of = links(&[("α", "β"), ("β", "α"), ("γ", "γ"), ("δ", "α"), ("ε", "ζ")]);
        let cyclic = words_on_cycles(&parent_of);
        let mut found: Vec<&str> = cyclic.iter().map(String::as_str).collect();
        found.sort();
        assert_eq!(found, ["α", "β", "γ"]);
    }

    #[test]
    fn root_follows_the_whole_chain() {
        let parent_of = links(&[("Γιωργάκης", "Γιώργος"), ("Γιώργος", "Γεώργιος")]);
        assert_eq!(root_of(&"Γιωργάκης".to_string(), &parent_of), "Γεώργιος");
        assert_eq!(root_of(&"Γεώργιος".to_string(), &parent_of), "Γεώργιος");
    }
}
