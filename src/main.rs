// Lemma - Greek dictionary generator. CLI entry point.

use clap::Parser;
use lemma::generator::{self, GeneratorOptions};

#[derive(Parser, Debug)]
#[command(about = "Lemma - Greek Dictionary Generator. Writes lemma_greek_<source>/lemma_greek_<source>.db, a SQLite database.")]
struct Cli {
    /// Source Wiktionary language: 'en' (English) or 'el' (Greek).
    #[arg(short = 's', long = "source", default_value = "en")]
    source: String,

    /// Limit to first PERCENT% of words (for testing).
    #[arg(short = 'l', long = "limit")]
    limit: Option<f64>,

    /// Max searchable inflected forms per headword. Default: 255
    #[arg(short = 'i', long = "inflections")]
    inflections: Option<usize>,
}

fn main() {
    let cli = Cli::parse();

    if cli.source != "en" && cli.source != "el" {
        eprintln!("Error: source must be 'en' or 'el'");
        std::process::exit(1);
    }

    if let Some(l) = cli.limit {
        if l <= 0.0 || l > 100.0 {
            eprintln!("Error: Limit must be between 0 and 100");
            std::process::exit(1);
        }
    }

    let opts = GeneratorOptions {
        source_lang: cli.source,
        limit_percent: cli.limit,
        max_inflections: cli.inflections,
    };

    if let Err(e) = generator::run(opts) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
