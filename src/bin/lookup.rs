// Look up words in the SQLite dictionary built with `lemma` (or `make sqlite`).
//
// Usage:
//     cargo run --release --bin lookup -- σπίτι
//     cargo run --release --bin lookup -- spiti        # Greeklish
//     cargo run --release --bin lookup -- --en house   # English -> Greek
//     cargo run --release --bin lookup                 # interactive
//     cargo run --release --bin lookup -- --check tests/golden_queries.tsv
//
// The check file has one `query<TAB>expected headword[<TAB>N]` per line (#
// starts a comment); each query passes when the expected headword is among
// the top N results (default 1), compared accent- and case-insensitively. A
// query prefixed with `en:` is looked up English -> Greek.

use clap::Parser;
use lemma::greek_keys;
use lemma::lookup::{self, Hit, MatchKind};
use rusqlite::{Connection, OpenFlags};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(about = "Look up Greek words (Greek or Greeklish) in the lemma SQLite database.")]
struct Cli {
    /// Database to query.
    #[arg(long = "db", default_value = "lemma_greek_en/lemma_greek_en.db")]
    db: PathBuf,

    /// Maximum number of headwords to show.
    #[arg(short = 'n', long = "limit", default_value_t = 5)]
    limit: usize,

    /// Run the golden queries in this file instead of looking up words.
    #[arg(long = "check", value_name = "FILE")]
    check: Option<PathBuf>,

    /// Look up English words and show their Greek equivalents.
    #[arg(long = "en", default_value_t = false)]
    english: bool,

    /// Word to look up. Without one, reads queries from stdin.
    query: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let conn = match Connection::open_with_flags(&cli.db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Cannot open {}: {} (build it with `make sqlite`)", cli.db.display(), e);
            return ExitCode::FAILURE;
        }
    };

    if let Some(path) = &cli.check {
        return run_check(&conn, path);
    }

    let print = if cli.english { print_english } else { print_lookup };

    if !cli.query.is_empty() {
        print(&conn, &cli.query.join(" "), cli.limit);
        return ExitCode::SUCCESS;
    }

    let stdin = io::stdin();
    loop {
        print!("> ");
        let _ = io::stdout().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if !line.trim().is_empty() {
            print(&conn, &line, cli.limit);
        }
    }
    ExitCode::SUCCESS
}

fn print_lookup(conn: &Connection, query: &str, limit: usize) {
    let hits = match lookup::search(conn, query, limit) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Query failed: {}", e);
            return;
        }
    };
    if hits.is_empty() {
        println!("No matches for {:?}", query.trim());
        return;
    }
    for hit in &hits {
        println!("{}", headline(hit));
        for e in lookup::entries(conn, hit.headword_id).unwrap_or_default() {
            let pos = match &e.head {
                Some(h) => format!("{}, {}", e.pos_display, h),
                None => e.pos_display.clone(),
            };
            let glosses: Vec<&str> = e.glosses.iter().take(3).map(|g| g.as_str()).collect();
            println!("    {}: {}", pos, glosses.join("; "));
        }
    }
}

fn print_english(conn: &Connection, query: &str, limit: usize) {
    let hits = match lookup::search_english(conn, query, limit) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Query failed: {}", e);
            return;
        }
    };
    if hits.is_empty() {
        println!("No matches for {:?}", query.trim());
        return;
    }
    if hits[0].full_text {
        println!("(no direct match; glosses mentioning {:?})", query.trim());
    }
    let width = hits.iter().map(|h| h.greek.chars().count()).max().unwrap_or(0);
    for hit in &hits {
        let mut detail = hit.gloss.as_deref().map(|g| shorten(g, 70)).unwrap_or_default();
        if let Some(note) = &hit.note {
            detail = if detail.is_empty() { format!("[{}]", note) } else { format!("{}  [{}]", detail, note) };
        }
        let pad = width - hit.greek.chars().count();
        println!("  {}{}  {}", hit.greek, " ".repeat(pad), detail);
    }
}

fn shorten(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars - 1).collect();
    format!("{}…", cut.trim_end())
}

fn headline(hit: &Hit) -> String {
    let mut line = hit.word.clone();
    if hit.matched_form != hit.word {
        line.push_str(&format!("  ← {}", hit.matched_form));
    }
    if hit.kind == MatchKind::Phonetic {
        line.push_str("  (by sound)");
    }
    line
}

fn run_check(conn: &Connection, path: &PathBuf) -> ExitCode {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Cannot read {}: {}", path.display(), e);
            return ExitCode::FAILURE;
        }
    };

    let mut passed = 0;
    let mut failed = 0;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').map(|f| f.trim()).collect();
        let (query, expected, within) = match fields.as_slice() {
            [q, e] => (*q, *e, 1),
            [q, e, n] if n.parse::<usize>().is_ok() => (*q, *e, n.parse().unwrap()),
            _ => {
                eprintln!("Skipping malformed line (expected query<TAB>headword[<TAB>N]): {}", line);
                continue;
            }
        };
        let (words, got) = match query.strip_prefix("en:") {
            Some(english) => {
                let hits = lookup::search_english(conn, english, within).unwrap_or_default();
                let got = hits.first().map(|h| h.greek.clone());
                (hits.into_iter().map(|h| h.greek).collect::<Vec<_>>(), got)
            }
            None => {
                let hits = lookup::search(conn, query, within).unwrap_or_default();
                let got = hits.first().map(headline);
                (hits.into_iter().map(|h| h.word).collect(), got)
            }
        };
        if words.iter().any(|w| greek_keys::norm(w) == greek_keys::norm(expected)) {
            passed += 1;
        } else {
            failed += 1;
            let got = got.unwrap_or_else(|| "no match".to_string());
            let scope = if within == 1 { String::new() } else { format!(" in top {}", within) };
            println!("FAIL  {:<16} expected {}{:<10} got {}", query, expected, scope, got);
        }
    }

    println!("{} passed, {} failed", passed, failed);
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
