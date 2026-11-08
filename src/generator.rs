// Builds the dictionary: download, parse, merge in Dilemma's inflections, then
// write the SQLite database.

use crate::dictionary::{BuildParams, Dictionary};
use crate::dilemma::DilemmaInflections;
use crate::downloader::Downloader;
use crate::en_translations;
use crate::entry_processor::{EntryMap, EntryProcessor};
use crate::sqlite_export;
use crate::version::LEMMA_VERSION;
use chrono::Local;

pub struct GeneratorOptions {
    pub source_lang: String,
    pub limit_percent: Option<f64>,
    pub max_inflections: Option<usize>,
}

pub fn run(opts: GeneratorOptions) -> Result<(), Box<dyn std::error::Error>> {
    if opts.source_lang != "en" && opts.source_lang != "el" {
        return Err("Source language must be 'en' or 'el'".into());
    }

    let build_date = Local::now().format("%Y%m%d").to_string();
    let source_desc = if opts.source_lang == "en" { "English" } else { "Greek" };
    println!("Lemma v{} - Greek Dictionary Generator", LEMMA_VERSION);
    println!("  Source: {} Wiktionary", source_desc);
    if let Some(p) = opts.limit_percent {
        println!("  Word limit: {}% of entries", p);
    }

    let mut downloader = Downloader::new(&opts.source_lang);
    let (success, filename) = downloader.download();
    let Some(filename) = filename.filter(|_| success) else {
        return Err("Download failed".into());
    };

    let mut dilemma = DilemmaInflections::new();
    let mut processor =
        EntryProcessor::new(&opts.source_lang, opts.limit_percent, &filename, Some(&mut dilemma));
    processor.process();
    let extraction_date = downloader.extraction_date.or(processor.extraction_date.clone());
    let entries = std::mem::replace(&mut processor.entries, EntryMap::new());
    drop(processor);

    let params = BuildParams {
        source_lang: opts.source_lang.clone(),
        limit_percent: opts.limit_percent,
        max_inflections: opts.max_inflections,
    };
    let mut dictionary = Dictionary::new(entries, params, Some(&dilemma));
    println!("Preparing entries...");
    dictionary.prepare();

    let translations = match en_translations::find_english_dump() {
        Some(path) => en_translations::load(&path)?,
        None => {
            println!("English kaikki dump not found under KAIKKI_LOCAL_DIR/en/; English index uses Greek glosses only");
            Vec::new()
        }
    };

    std::fs::create_dir_all(&dictionary.output_dir)?;
    let db_path = dictionary.output_dir.join(format!("lemma_greek_{}.db", opts.source_lang));
    println!("\nWriting SQLite database...");
    let headwords = dictionary.export_headwords();
    let meta = [
        ("lemma_version", LEMMA_VERSION.to_string()),
        ("source", format!("{} Wiktionary via kaikki.org", source_desc)),
        ("source_lang", opts.source_lang.clone()),
        ("build_date", build_date),
        ("extraction_date", extraction_date.clone().unwrap_or_default()),
    ];
    let stats = sqlite_export::write_database(&db_path, &headwords, &translations, &meta)?;
    let size_mb = std::fs::metadata(&db_path).map(|m| m.len() as f64 / 1_048_576.0).unwrap_or(0.0);
    println!(
        "  {} headwords, {} entries, {} senses, {} searchable forms ({:.1} MB)",
        stats.headwords, stats.entries, stats.senses, stats.forms, size_mb
    );
    println!(
        "  {} English terms; {} translations linked to headwords, {} kept as text only",
        stats.en_terms, stats.translations_linked, stats.translations_unlinked
    );
    println!("  {} inflection tables, {} cells", stats.inflection_tables, stats.inflections);
    println!("  Wrote {}", db_path.display());
    if let Some(d) = extraction_date {
        println!("Wiktionary extraction date: {}", d);
    }
    Ok(())
}
