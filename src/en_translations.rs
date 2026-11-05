// Loads English->Greek pairs from English Wiktionary's translation tables.
//
// The English kaikki dump (kaikki.org/dictionary/English/, ~3.4 GB, or ~0.5
// GB gzipped) lists translations on English entries, both entry-wide and per
// sense. Only the Modern Greek ones (`code: "el"`) are kept. The dump is
// optional: without it the English index is built from Greek glosses alone.
//
// Expected at `$KAIKKI_LOCAL_DIR/en/kaikki.org-dictionary-English.jsonl.gz`
// (or the same name without `.gz`).

use crate::downloader::kaikki_local_dir;
use flate2::read::MultiGzDecoder;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

const ENGLISH_DUMP_STEM: &str = "en/kaikki.org-dictionary-English.jsonl";

#[derive(Debug, Clone)]
pub struct EnTranslation {
    /// English headword the translation table belongs to.
    pub english: String,
    pub greek: String,
    /// Which sense of the English word, e.g. "financial institution".
    pub sense: Option<String>,
}

pub fn find_english_dump() -> Option<PathBuf> {
    let dir = kaikki_local_dir()?;
    [format!("{}.gz", ENGLISH_DUMP_STEM), ENGLISH_DUMP_STEM.to_string()]
        .into_iter()
        .map(|rel| dir.join(rel))
        .find(|p| p.exists())
}

/// Read every Modern Greek translation from the dump, deduplicated by
/// (English word, Greek word) and keeping the first sense label seen, which
/// is usually the primary sense.
pub fn load(path: &Path) -> std::io::Result<Vec<EnTranslation>> {
    println!("Loading English->Greek translations from {}...", path.display());
    let start = Instant::now();

    let file = File::open(path)?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(MultiGzDecoder::new(file))
    } else {
        Box::new(file)
    };

    let mut out: Vec<EnTranslation> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut lines = 0usize;
    for line in BufReader::with_capacity(1 << 20, reader).lines() {
        let line = line?;
        lines += 1;
        // Cheap pre-filter: most entries have no Greek translation.
        if !line.contains("\"code\": \"el\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if entry.get("lang_code").and_then(|v| v.as_str()) != Some("en") {
            continue;
        }
        let Some(english) = entry.get("word").and_then(|v| v.as_str()) else { continue };
        if is_affix(english) {
            continue;
        }

        let entry_level = entry.get("translations").and_then(|v| v.as_array()).into_iter().flatten();
        let sense_level = entry
            .get("senses")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("translations").and_then(|v| v.as_array()))
            .flatten();

        for t in entry_level.chain(sense_level) {
            if t.get("code").and_then(|v| v.as_str()) != Some("el") {
                continue;
            }
            let Some(greek) = t.get("word").and_then(|v| v.as_str()).map(str::trim) else { continue };
            if greek.is_empty() || is_affix(greek) || !seen.insert((english.to_string(), greek.to_string())) {
                continue;
            }
            out.push(EnTranslation {
                english: english.to_string(),
                greek: greek.to_string(),
                sense: t.get("sense").and_then(|v| v.as_str()).map(str::to_string).filter(|s| !s.is_empty()),
            });
        }
    }

    println!(
        "Loaded {} English->Greek translations from {} entries in {:.1}s",
        out.len(),
        lines,
        start.elapsed().as_secs_f64()
    );
    Ok(out)
}

// Prefixes and suffixes ("odonto-", "-logy") are not words to look up.
fn is_affix(word: &str) -> bool {
    word.starts_with('-') || word.ends_with('-')
}
