// Build the Lemma Greek dictionary as a SQLite database (no Kindle output).
//
// Runs the same download -> parse -> Dilemma merge -> form-of folding ->
// inflection ranking pipeline as the `lemma` binary, then writes
// lemma_greek_<source>.sqlite instead of EPUB/MOBI/StarDict.
//
// Must be run from the repository root: the supporting tables in data/
// (polytonic variants, lemma equivalences, frequency list) are found by
// relative path, and are silently skipped otherwise.
//
//   cargo run --release --bin lemma-sqlite
//   cargo run --release --bin lemma-sqlite -- -l 5 -o /tmp/test.sqlite

use chrono::Local;
use clap::Parser;
use lemma::dilemma::DilemmaInflections;
use lemma::downloader::Downloader;
use lemma::entry_processor::{EntryMap, EntryProcessor};
use lemma::html_gen::{BuildParams, HtmlGenerator};
use lemma::sqlite::{write_sqlite, SqliteOptions};
use lemma::version::LEMMA_VERSION;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(about = "Build the Lemma Greek dictionary as a SQLite database.")]
struct Cli {
    /// Source Wiktionary language: 'en' (English) or 'el' (Greek).
    #[arg(short = 's', long = "source", default_value = "en")]
    source: String,

    /// Limit to first PERCENT% of words (for testing).
    #[arg(short = 'l', long = "limit")]
    limit: Option<f64>,

    /// Output path. Default: lemma_greek_<source>.sqlite (or
    /// lemma_greek_<source>_<N>pct.sqlite with --limit).
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,

    /// Cap ranked inflections per headword. Default: keep all.
    #[arg(long = "max-inflections")]
    max_inflections: Option<usize>,

    /// Cap polytonic variants per headword. Default: keep all.
    #[arg(long = "max-polytonic")]
    max_polytonic: Option<usize>,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if cli.source != "en" && cli.source != "el" {
        return Err("source must be 'en' or 'el'".into());
    }
    if let Some(l) = cli.limit {
        if l <= 0.0 || l > 100.0 {
            return Err("limit must be between 0 and 100".into());
        }
    }
    if !Path::new("data").is_dir() {
        eprintln!(
            "Warning: no data/ directory here. Run from the lemma repo root, or polytonic \
             variants, lemma equivalences and frequency ranking will be missing."
        );
    }

    let output = cli.output.clone().unwrap_or_else(|| {
        let mut name = format!("lemma_greek_{}", cli.source);
        if let Some(p) = cli.limit {
            name = format!("{}_{}pct", name, p);
        }
        PathBuf::from(format!("{}.sqlite", name))
    });

    println!("Lemma v{} - Greek dictionary -> SQLite", LEMMA_VERSION);
    let build_date = Local::now().format("%Y-%m-%d").to_string();

    let mut downloader = Downloader::new(&cli.source);
    let (ok, filename) = downloader.download();
    let filename = match (ok, filename) {
        (true, Some(f)) => f,
        _ => return Err("download of Kaikki data failed".into()),
    };

    let mut dilemma = DilemmaInflections::new();
    let mut processor = EntryProcessor::new(&cli.source, cli.limit, &filename, Some(&mut dilemma));
    processor.process();
    let extraction_date = downloader.extraction_date.clone().or(processor.extraction_date.clone());
    let entries = std::mem::replace(&mut processor.entries, EntryMap::new());
    drop(processor);

    let params = BuildParams {
        source_lang: cli.source.clone(),
        build_date: build_date.clone(),
        extraction_date: extraction_date.clone(),
        limit_percent: cli.limit,
        max_inflections: cli.max_inflections,
        front_matter: serde_json::json!({}),
    };
    // HtmlGenerator holds the shared form-ranking logic; nothing is written
    // to disk unless create_output_files() is called, which we don't.
    let mut generator = HtmlGenerator::new(entries, params, Some(&dilemma));
    println!("Preparing entries...");
    generator.prepare();

    let source_desc = if cli.source == "en" { "English Wiktionary" } else { "Greek Wiktionary" };
    let mut meta: Vec<(String, String)> = vec![
        ("generator".into(), format!("lemma v{}", LEMMA_VERSION)),
        ("build_date".into(), build_date),
        ("source".into(), format!("{} via kaikki.org", source_desc)),
        ("source_lang".into(), cli.source.clone()),
        (
            "license".into(),
            "CC BY-SA 4.0 (dictionary content derived from Wiktionary)".into(),
        ),
        ("license_url".into(), "https://creativecommons.org/licenses/by-sa/4.0/".into()),
        ("polytonic_source".into(), if dilemma.has_polytonic_ranked() {
            "dilemma mg_polytonic_ranked.json (attested in Greek Wikisource)".into()
        } else {
            "rule-generated (mg_polytonic_ranked.json not found)".into()
        }),
        ("inflection_ranking".into(), if dilemma.has_ranked_forms() {
            "dilemma mg_ranked_forms.json".into()
        } else {
            "OpenSubtitles frequency (data/el_full.txt)".into()
        }),
    ];
    if let Some(d) = &extraction_date {
        meta.push(("wiktionary_extraction_date".into(), d.clone()));
    }
    if let Some(p) = cli.limit {
        meta.push(("limit_percent".into(), p.to_string()));
    }

    println!("Writing {}...", output.display());
    let start = Instant::now();
    let opts = SqliteOptions {
        max_inflections: cli.max_inflections,
        max_polytonic: cli.max_polytonic,
        meta,
    };
    let report = write_sqlite(&generator, &dilemma.equivalences, &output, &opts)?;

    let size = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    println!(
        "Done in {:.1}s: {} headwords, {} entries, {} senses, {} forms, {} cross-references ({:.1} MB)",
        start.elapsed().as_secs_f64(),
        report.headwords,
        report.entries,
        report.senses,
        report.forms,
        report.xrefs,
        size as f64 / 1024.0 / 1024.0
    );
    Ok(())
}
