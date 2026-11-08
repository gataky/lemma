// Keeps Wiktionary's inflection tables with their grammatical labels.
//
// `collect_inflections_from` flattens a kaikki entry's `forms` into searchable
// spellings for the Kindle lookup index, which drops the tags and anything with
// a space (θα γράψω). This module reads the same `forms` for the SQLite export,
// keeping every cell's tags and which table it came from, so a consumer can lay
// out a conjugation or declension.
//
// Cell text is stored as Wiktionary has it, typos and all. Three things are not
// cells and are handled here: table markers (`table-tags`,
// `inflection-template`) become table metadata, romanizations are dropped, and
// sentences that the parser reports as forms become the table's note.

use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

/// `kind` of the table holding the forms from the headword line: principal
/// parts (past, passive), plural, feminine, alternative spellings. Other kinds
/// are kaikki's `source`: `declension`, `conjugation`, `inflection`.
pub const HEAD_KIND: &str = "head";

const NO_LABEL: &str = "no-table-tags";

#[derive(Debug, Clone, PartialEq)]
pub struct InflectedForm {
    pub form: String,
    /// Sorted, space-separated: `active first-person indicative past perfective singular`.
    pub tags: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InflectionTable {
    pub kind: String,
    /// Set when the table was moved here from a form-of entry that was merged
    /// into this headword: the word it inflects (καφεδάκι under καφές).
    pub word: Option<String>,
    /// Wiktionary's label for the whole table: `perfect`, `deponent`, ...
    pub label: Option<String>,
    /// Wiktionary template that produced it: `el-conjug-1st`, `el-nF-α-ες-3b2`.
    pub template: Option<String>,
    pub note: Option<String>,
    pub forms: Vec<InflectedForm>,
}

impl InflectionTable {
    fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_string(),
            word: None,
            label: None,
            template: None,
            note: None,
            forms: Vec::new(),
        }
    }

    // One note arrives as several "forms", split at its commas, and repeats
    // for each voice.
    fn add_note(&mut self, text: &str) {
        match &mut self.note {
            Some(note) if note.contains(text) => {}
            Some(note) => {
                note.push_str(", ");
                note.push_str(text);
            }
            None => self.note = Some(text.to_string()),
        }
    }
}

// Two Latin words: table headers and sentences that the parser reports as
// forms. A single one is a real cell: a loanword (DVD) or a typo (αφtιασίδωτο).
fn note_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\b[A-Za-z]{2,}\b.*\b[A-Za-z]{2,}\b").unwrap())
}

fn superlative_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(τατος|ιστος)$").unwrap())
}

fn sorted_tags(form: &Value) -> Vec<String> {
    let mut tags: Vec<String> = form
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| tags.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    // `error-*` tags are kaikki's parser reporting on itself, not grammar.
    tags.retain(|t| !t.starts_with("error-"));
    tags.sort();
    tags.dedup();
    tags
}

/// Read a kaikki entry's `forms` into tables: the headword-line forms first,
/// then the inflection tables in Wiktionary's order.
pub fn parse_tables(entry: &Value) -> Vec<InflectionTable> {
    let Some(forms) = entry.get("forms").and_then(Value::as_array) else {
        return Vec::new();
    };

    let mut head = InflectionTable::new(HEAD_KIND);
    let mut tables: Vec<InflectionTable> = Vec::new();

    for form in forms {
        let text = form.get("form").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let tags = sorted_tags(form);
        let has_tag = |name: &str| tags.iter().any(|t| t == name);
        let source = form.get("source").and_then(Value::as_str);

        if has_tag("romanization") {
            continue;
        }
        if has_tag("table-tags") {
            let mut table = InflectionTable::new(source.unwrap_or(HEAD_KIND));
            table.label = (text != NO_LABEL).then(|| text.to_string());
            tables.push(table);
            continue;
        }
        if has_tag("inflection-template") {
            if let Some(table) = tables.last_mut() {
                table.template.get_or_insert_with(|| text.to_string());
            }
            continue;
        }

        let table = match source {
            None => &mut head,
            Some(source) => {
                if tables.is_empty() {
                    tables.push(InflectionTable::new(source));
                }
                tables.last_mut().unwrap()
            }
        };
        if note_re().is_match(text) {
            table.add_note(text);
        } else {
            table.forms.push(InflectedForm { form: text.to_string(), tags: tags.join(" ") });
        }
    }

    resolve_degrees(&mut tables);

    let mut out: Vec<InflectionTable> = Vec::new();
    if !head.forms.is_empty() || head.note.is_some() {
        out.push(head);
    }
    for table in tables {
        if !table.forms.is_empty() {
            out.push(table);
        } else if let Some(note) = &table.note {
            // A footnote parsed as a table of its own ("Derivations: ...").
            match out.last_mut() {
                Some(previous) if previous.kind != HEAD_KIND => previous.add_note(note),
                _ => out.push(table),
            }
        }
    }
    out
}

// kaikki tags the comparative and superlative tables of an adjective alike,
// both `comparative superlative`, so ομορφότερος and ομορφότατος would be
// indistinguishable. The comparative table comes first. The ending catches the
// few adjectives with only a superlative table (άκρος -> ακρότατος), and order
// alone covers source typos (θαυμαστότatος).
fn resolve_degrees(tables: &mut [InflectionTable]) {
    let is_ambiguous = |tags: &str| {
        let mut parts = tags.split(' ');
        parts.clone().any(|t| t == "comparative") && parts.any(|t| t == "superlative")
    };
    let mut seen_comparative = false;
    for table in tables {
        let Some(first) = table.forms.iter().find(|f| is_ambiguous(&f.tags)) else {
            continue;
        };
        let superlative = seen_comparative || superlative_re().is_match(&first.form);
        seen_comparative |= !superlative;
        let wrong = if superlative { "comparative" } else { "superlative" };
        for f in table.forms.iter_mut().filter(|f| is_ambiguous(&f.tags)) {
            f.tags = f.tags.split(' ').filter(|t| *t != wrong).collect::<Vec<_>>().join(" ");
        }
    }
}

/// Add `incoming` to an entry's tables. Headword-line forms pool into one table
/// per word, and a table identical to one already present is dropped: two
/// etymologies of a word usually inflect the same way.
pub fn merge_tables(existing: &mut Vec<InflectionTable>, incoming: Vec<InflectionTable>) {
    for table in incoming {
        if table.kind == HEAD_KIND {
            if let Some(head) = existing.iter_mut().find(|t| t.kind == HEAD_KIND && t.word == table.word) {
                for form in table.forms {
                    if !head.forms.contains(&form) {
                        head.forms.push(form);
                    }
                }
                if let Some(note) = &table.note {
                    head.add_note(note);
                }
                continue;
            }
        }
        if !existing.contains(&table) {
            existing.push(table);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cell(form: &str, tags: &str, source: &str) -> Value {
        json!({ "form": form, "tags": tags.split(' ').collect::<Vec<_>>(), "source": source })
    }

    fn marker(label: &str, template: &str, source: &str) -> Vec<Value> {
        vec![cell(label, "table-tags", source), cell(template, "inflection-template", source)]
    }

    fn tables(forms: Vec<Value>) -> Vec<InflectionTable> {
        parse_tables(&json!({ "forms": forms }))
    }

    fn lookup<'a>(table: &'a InflectionTable, tags: &str) -> Vec<&'a str> {
        table.forms.iter().filter(|f| f.tags == tags).map(|f| f.form.as_str()).collect()
    }

    #[test]
    fn keeps_tags_order_and_multi_word_forms() {
        let mut forms = marker("no-table-tags", "el-conjug-1st", "conjugation");
        forms.push(cell("έγραψα", "singular first-person past", "conjugation"));
        forms.push(cell("θα γράψω", "future first-person singular", "conjugation"));
        let t = tables(forms);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, "conjugation");
        assert_eq!(t[0].template.as_deref(), Some("el-conjug-1st"));
        assert_eq!(t[0].label, None);
        assert_eq!(t[0].forms[0].form, "έγραψα");
        assert_eq!(t[0].forms[0].tags, "first-person past singular");
        assert_eq!(t[0].forms[1].form, "θα γράψω");
    }

    #[test]
    fn headword_line_forms_go_in_their_own_table() {
        let mut forms = vec![
            json!({ "form": "gráfo", "tags": ["romanization"] }),
            json!({ "form": "έγραψα", "tags": ["past"] }),
        ];
        forms.extend(marker("no-table-tags", "el-conjug-1st", "conjugation"));
        forms.push(cell("γράφω", "present", "conjugation"));
        let t = tables(forms);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].kind, HEAD_KIND);
        assert_eq!(lookup(&t[0], "past"), ["έγραψα"]);
        assert_eq!(t[1].kind, "conjugation");
    }

    #[test]
    fn table_label_is_kept_and_placeholder_is_not() {
        let mut forms = marker("deponent", "el-conjug-1st", "conjugation");
        forms.push(cell("έρχομαι", "present", "conjugation"));
        assert_eq!(tables(forms)[0].label.as_deref(), Some("deponent"));
    }

    #[test]
    fn placeholder_for_a_missing_form_and_abbreviated_endings_are_kept() {
        let mut forms = marker("no-table-tags", "el-conjug-1st", "conjugation");
        forms.push(cell("-", "participle present", "conjugation"));
        forms.push(cell("γραμμένος", "participle past", "conjugation"));
        forms.push(cell("‑η", "participle past", "conjugation"));
        let t = tables(forms);
        assert_eq!(lookup(&t[0], "participle present"), ["-"]);
        assert_eq!(lookup(&t[0], "participle past"), ["γραμμένος", "‑η"]);
    }

    #[test]
    fn split_notes_are_rejoined_once_and_are_not_cells() {
        let mut forms = marker("no-table-tags", "el-conjug-1st", "conjugation");
        for tag in ["active", "passive"] {
            for part in ["Formed using present", "dependent (for simple past)"] {
                forms.push(cell(part, &format!("subjunctive {tag}"), "conjugation"));
            }
        }
        forms.push(cell("γράφω", "present", "conjugation"));
        let t = tables(forms);
        assert_eq!(t[0].forms.len(), 1);
        assert_eq!(t[0].note.as_deref(), Some("Formed using present, dependent (for simple past)"));
    }

    #[test]
    fn single_latin_words_are_cells_not_notes() {
        let mut forms = marker("no-table-tags", "el-nN", "declension");
        forms.push(cell("DVD", "nominative singular", "declension"));
        forms.push(cell("αφtιασίδωτο", "neuter", "declension"));
        assert_eq!(tables(forms)[0].forms.len(), 2);
    }

    #[test]
    fn note_only_table_becomes_a_note_on_the_previous_table() {
        let mut forms = marker("no-table-tags", "el-decl-adj", "declension");
        forms.push(cell("καλός", "masculine", "declension"));
        forms.extend(marker("no-table-tags", "el-decl-adj", "declension"));
        forms.push(json!({ "form": "Derivations: relative superlative", "source": "declension" }));
        let t = tables(forms);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].note.as_deref(), Some("Derivations: relative superlative"));
    }

    #[test]
    fn parser_error_tags_are_dropped() {
        let mut forms = marker("no-table-tags", "el-conjug-1st", "conjugation");
        forms.push(cell("φύομαι", "error-unrecognized-form present", "conjugation"));
        assert_eq!(tables(forms)[0].forms[0].tags, "present");
    }

    fn adjective(masculines: &[&str]) -> Vec<InflectionTable> {
        let mut forms = marker("no-table-tags", "el-decl-adj", "declension");
        forms.push(cell("όμορφος", "masculine nominative singular", "declension"));
        for m in masculines {
            forms.extend(marker("no-table-tags", "el-decl-adj", "declension"));
            forms.push(cell(m, "comparative masculine nominative singular superlative", "declension"));
        }
        tables(forms)
    }

    const COMPARATIVE: &str = "comparative masculine nominative singular";
    const SUPERLATIVE: &str = "masculine nominative singular superlative";

    #[test]
    fn comparative_and_superlative_tables_are_told_apart() {
        let t = adjective(&["ομορφότερος", "ομορφότατος"]);
        assert_eq!(t[0].forms[0].tags, "masculine nominative singular");
        assert_eq!(lookup(&t[1], COMPARATIVE), ["ομορφότερος"]);
        assert_eq!(lookup(&t[2], SUPERLATIVE), ["ομορφότατος"]);
    }

    #[test]
    fn lone_degree_table_is_identified_by_its_ending() {
        assert_eq!(lookup(&adjective(&["κοντύτερος"])[1], COMPARATIVE), ["κοντύτερος"]);
        assert_eq!(lookup(&adjective(&["ακρότατος"])[1], SUPERLATIVE), ["ακρότατος"]);
    }

    #[test]
    fn misspelled_superlative_is_identified_by_position() {
        let t = adjective(&["αυθεντικότερος", "αυθεντικότος"]);
        assert_eq!(lookup(&t[2], SUPERLATIVE), ["αυθεντικότος"]);
    }

    #[test]
    fn merging_keeps_a_headword_line_note() {
        let note = |text: &str| tables(vec![json!({ "form": text, "tags": ["canonical"] })]);
        let mut existing = tables(vec![json!({ "form": "x", "tags": ["alternative"] })]);
        merge_tables(&mut existing, note("found chiefly in the present"));
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].note.as_deref(), Some("found chiefly in the present"));
    }

    #[test]
    fn merging_pools_head_forms_and_drops_identical_tables() {
        let mut a = marker("no-table-tags", "el-nM", "declension");
        a.push(cell("καρπός", "nominative singular", "declension"));
        let mut b = a.clone();
        b.push(cell("καρποί", "nominative plural", "declension"));

        let with_head = |mut forms: Vec<Value>, head: &str| {
            forms.insert(0, json!({ "form": head, "tags": ["alternative"] }));
            tables(forms)
        };

        let mut existing = with_head(a.clone(), "x");
        merge_tables(&mut existing, with_head(a.clone(), "y"));
        assert_eq!(existing.len(), 2, "identical declension dropped");
        assert_eq!(lookup(&existing[0], "alternative"), ["x", "y"]);

        merge_tables(&mut existing, tables(b));
        assert_eq!(existing.len(), 3, "different declension kept");
    }
}
