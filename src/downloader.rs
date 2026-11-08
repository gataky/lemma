// Finds the kaikki dump of Greek Wiktionary: a local copy, a previous
// download, or a fresh download from kaikki.org.

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::json;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub struct Downloader {
    pub source_lang: String,
    pub extraction_date: Option<String>,
}

const KAIKKI_URL_EN: &str = "https://kaikki.org/dictionary/Greek/kaikki.org-dictionary-Greek.jsonl";
const KAIKKI_URL_EL: &str = "https://kaikki.org/elwiktionary/Greek/kaikki.org-dictionary-Greek.jsonl";

const KAIKKI_INDEX_EN: &str = "https://kaikki.org/dictionary/Greek/";
const KAIKKI_INDEX_EL: &str = "https://kaikki.org/elwiktionary/Greek/";

const LOCAL_KAIKKI_EN: &str = "en-el/kaikki.org-dictionary-Greek-words.jsonl";
const LOCAL_KAIKKI_EL: &str = "el/kaikki.org-dictionary-Greek.jsonl";

impl Downloader {
    pub fn new(source_lang: &str) -> Self {
        Self {
            source_lang: source_lang.to_string(),
            extraction_date: None,
        }
    }

    fn primary_url(&self) -> &'static str {
        if self.source_lang == "en" { KAIKKI_URL_EN } else { KAIKKI_URL_EL }
    }
    fn index_url(&self) -> &'static str {
        if self.source_lang == "en" { KAIKKI_INDEX_EN } else { KAIKKI_INDEX_EL }
    }
    fn local_kaikki_rel(&self) -> &'static str {
        if self.source_lang == "en" { LOCAL_KAIKKI_EN } else { LOCAL_KAIKKI_EL }
    }

    /// Find the kaikki dump, in order: a local dump under `KAIKKI_LOCAL_DIR`
    /// (read in place), a previous download in the working directory, then
    /// kaikki.org. Returns (success, filename) and records the Wiktionary
    /// extraction date in `self.extraction_date`.
    pub fn download(&mut self) -> (bool, Option<String>) {
        if let Some(local_path) = self.find_local_kaikki_file() {
            println!("Using local kaikki dump: {}", local_path.display());
            self.extraction_date = load_sidecar(local_path.to_str().unwrap_or(""))
                .or_else(|| mtime_as_iso(&local_path));
            if let Some(d) = &self.extraction_date {
                println!("  Extraction date (local dump): {}", d);
            }
            return (true, Some(local_path.to_string_lossy().into_owned()));
        }

        let target_filename = format!("greek_data_{}.jsonl", self.source_lang);
        if fs::metadata(&target_filename).map(|m| m.len() > 0).unwrap_or(false) {
            let line_count = count_lines(&target_filename);
            println!("Using previous download: {} ({} lines)", target_filename, line_count);
            self.extraction_date = load_sidecar(&target_filename);
            if let Some(d) = &self.extraction_date {
                println!("  Extraction date (from sidecar): {}", d);
            }
            return (true, Some(target_filename));
        }

        let lang_desc = if self.source_lang == "en" { "English" } else { "Greek" };
        println!("Downloading Greek data from {} Wiktionary via Kaikki...", lang_desc);
        match self.download_from_url(self.primary_url(), &target_filename) {
            Ok(http_date) => {
                self.extraction_date = http_date.or_else(|| self.scrape_index_page_date());
                if let Some(d) = &self.extraction_date {
                    println!("  Extraction date (Kaikki): {}", d);
                    let url = self.primary_url().to_string();
                    self.write_sidecar(&target_filename, &url);
                } else {
                    println!("  Warning: could not determine Kaikki extraction date");
                }
                (true, Some(target_filename))
            }
            Err(e) => {
                println!("Error: download failed: {}", e);
                println!(
                    "Download {} yourself and put it at $KAIKKI_LOCAL_DIR/{}",
                    self.primary_url(),
                    self.local_kaikki_rel()
                );
                (false, None)
            }
        }
    }

    fn find_local_kaikki_file(&self) -> Option<PathBuf> {
        let full_path = kaikki_local_dir()?.join(self.local_kaikki_rel());
        if !full_path.exists() { return None; }
        Some(full_path)
    }

    fn download_from_url(&self, url: &str, filename: &str) -> Result<Option<String>, String> {
        println!("Attempting to download from: {}", url);

        let start = Instant::now();
        let client = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| e.to_string())?;

        let resp = client.get(url).send().map_err(|e| e.to_string())?;
        let status = resp.status();
        println!("Response code: {}", status);
        if !status.is_success() {
            return Err(format!("HTTP {}", status));
        }

        let last_modified = resp.headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        if let Some(lm) = &last_modified {
            println!("Last-Modified: {}", lm);
        }
        let extraction_date = last_modified.as_deref().and_then(parse_http_date);

        let total_size = resp.content_length().unwrap_or(0);
        if total_size > 0 {
            println!("Content-Length: {} bytes ({:.2} MB)", total_size, total_size as f64 / 1024.0 / 1024.0);
        }

        let bytes = resp.bytes().map_err(|e| e.to_string())?;

        // Kaikki URLs sometimes return gzip; detect and decompress
        let content: Vec<u8> = if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
            let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
            let mut out = Vec::new();
            decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
            out
        } else {
            bytes.to_vec()
        };

        let mut f = File::create(filename).map_err(|e| e.to_string())?;
        f.write_all(&content).map_err(|e| e.to_string())?;

        let elapsed = start.elapsed().as_secs_f64();
        let mb = content.len() as f64 / 1024.0 / 1024.0;
        println!("Download complete: {:.2} MB in {:.2} seconds", mb, elapsed);

        let line_count = count_lines(filename);
        println!("Downloaded {} lines to {}", line_count, filename);
        Ok(extraction_date)
    }

    fn scrape_index_page_date(&self) -> Option<String> {
        let client = reqwest::blocking::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(30))
            .build().ok()?;
        let body = client.get(self.index_url()).send().ok()?.text().ok()?;
        let re = Regex::new(r"extracted on (\d{4}-\d{2}-\d{2})").unwrap();
        re.captures(&body).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
    }

    fn write_sidecar(&self, jsonl_path: &str, source_url: &str) {
        let Some(extraction) = &self.extraction_date else { return; };
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let meta = json!({
            "extraction_date": extraction,
            "source_url": source_url,
            "downloaded_at": today,
        });
        let sidecar = format!("{}.meta", jsonl_path);
        if let Ok(mut f) = File::create(&sidecar) {
            let _ = f.write_all(serde_json::to_string_pretty(&meta).unwrap_or_default().as_bytes());
            let _ = f.write_all(b"\n");
        }
    }
}

/// Directory of local kaikki dumps, from `KAIKKI_LOCAL_DIR` in the
/// environment or in `.env`.
pub fn kaikki_local_dir() -> Option<PathBuf> {
    let mut kaikki_dir = std::env::var("KAIKKI_LOCAL_DIR").unwrap_or_default();
    if kaikki_dir.is_empty() {
        if let Ok(contents) = fs::read_to_string(".env") {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                if let Some((k, v)) = line.split_once('=') {
                    if k.trim() == "KAIKKI_LOCAL_DIR" {
                        kaikki_dir = v.trim().to_string();
                    }
                }
            }
        }
    }
    if kaikki_dir.is_empty() { return None; }
    Some(PathBuf::from(kaikki_dir))
}

fn count_lines(filename: &str) -> usize {
    let Ok(f) = File::open(filename) else { return 0 };
    BufReader::new(f).lines().count()
}

fn parse_http_date(header: &str) -> Option<String> {
    let dt: std::time::SystemTime = httpdate::parse_http_date(header).ok()?;
    let dt: DateTime<Utc> = dt.into();
    Some(dt.format("%Y-%m-%d").to_string())
}

fn parse_yyyymmdd_as_iso(value: &str) -> Option<String> {
    if value.len() == 8 && value.chars().all(|c| c.is_ascii_digit()) {
        Some(format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..]))
    } else {
        None
    }
}

fn mtime_as_iso(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let dt: DateTime<chrono::Local> = mtime.into();
    Some(dt.format("%Y-%m-%d").to_string())
}

fn load_sidecar(jsonl_path: &str) -> Option<String> {
    let sidecar = format!("{}.meta", jsonl_path);
    let contents = fs::read_to_string(&sidecar).ok()?;
    let meta: serde_json::Value = serde_json::from_str(&contents).ok()?;
    let raw = meta.get("extraction_date")?.as_str()?;
    if raw.len() == 10 && raw.chars().nth(4) == Some('-') {
        return Some(raw.to_string());
    }
    parse_yyyymmdd_as_iso(raw).or_else(|| Some(raw.to_string()))
}
