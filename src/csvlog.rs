use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const HEADER: [&str; 5] = ["type", "url", "name", "hash", "is_dupe"];

pub const MEDIA: &str = "media";
pub const PAGE: &str = "page";

struct Row {
    kind: &'static str,
    url: String,
    name: String,
    hash: String,
    dupe: bool,
}

fn encode(rec: &[&str]) -> std::io::Result<Vec<u8>> {
    let mut w = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(Vec::new());
    w.write_record(rec)?;
    w.into_inner()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// --live-csv workspace: `type,url,name,hash,is_dupe`.
///   type = media | page
///   name = relative file name (media) or the page title (page)
///   hash = sha256 of the file / of the page's HTML
///   is_dupe = 1 when the content was already seen (media: file deleted)
/// Loaded on startup, then only ever appended to (each row is written, flushed and synced at once,
/// so an interrupted run leaves a complete record of everything finished so far).
pub struct CsvLog {
    file: Mutex<File>,
    media_urls: Mutex<HashSet<String>>,
    pages: Mutex<HashSet<String>>,
    /// page hash -> url of the first page with that content
    page_hashes: Mutex<HashMap<String, String>>,
    /// (hash, name) of every loaded media row with is_dupe=0
    loaded_hashes: Vec<(String, String)>,
    /// names of every loaded media row with is_dupe=0
    loaded_names: HashSet<String>,
    pub loaded_rows: usize,
    pub bad_rows: usize,
    /// an old 4-column file was converted (a copy was kept as <file>.legacy.bak)
    pub migrated: bool,
}

impl CsvLog {
    pub fn open(path: &Path) -> Result<CsvLog, String> {
        let err = |e: &dyn std::fmt::Display| format!("--live-csv {}: {}", path.display(), e);

        let mut rows: Vec<Row> = Vec::new();
        let mut bad = 0usize;
        let mut legacy = false;

        let existing_len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if existing_len > 0 {
            let mut rdr = csv::ReaderBuilder::new()
                .has_headers(false)
                .flexible(true)
                .from_path(path)
                .map_err(|e| err(&e))?;
            let mut first = true;
            for rec in rdr.records() {
                let Ok(rec) = rec else {
                    bad += 1;
                    continue;
                };
                if first {
                    first = false;
                    if rec.get(0) == Some("type") && rec.get(1) == Some("url") {
                        continue; // current header
                    }
                    if rec.get(0) == Some("url") && rec.get(1) == Some("filename") {
                        legacy = true; // old header: url,filename,hash,is_dupe
                        continue;
                    }
                }
                let f = |i: usize| rec.get(i).unwrap_or("").trim().to_string();
                let row = if legacy {
                    Row {
                        kind: MEDIA,
                        url: f(0),
                        name: f(1),
                        hash: f(2),
                        dupe: f(3) == "1",
                    }
                } else {
                    let kind = if f(0).eq_ignore_ascii_case(PAGE) {
                        PAGE
                    } else {
                        MEDIA
                    };
                    Row {
                        kind,
                        url: f(1),
                        name: f(2),
                        hash: f(3),
                        dupe: f(4) == "1",
                    }
                };
                if row.url.is_empty() && row.name.is_empty() {
                    bad += 1;
                    continue;
                }
                rows.push(row);
            }
        }

        // old 4-column file: keep a copy, rewrite it in the new format (nothing is lost)
        if legacy {
            let bak = with_suffix(path, ".legacy.bak");
            fs::copy(path, &bak).map_err(|e| err(&e))?;
            let tmp = with_suffix(path, ".tmp");
            let mut out = encode(&HEADER).map_err(|e| err(&e))?;
            for r in &rows {
                out.extend(
                    encode(&[
                        r.kind,
                        &r.url,
                        &r.name,
                        &r.hash,
                        if r.dupe { "1" } else { "0" },
                    ])
                    .map_err(|e| err(&e))?,
                );
            }
            fs::write(&tmp, &out).map_err(|e| err(&e))?;
            fs::rename(&tmp, path).map_err(|e| err(&e))?;
        }

        let mut media_urls = HashSet::new();
        let mut pages = HashSet::new();
        let mut page_hashes: HashMap<String, String> = HashMap::new();
        let mut loaded_hashes = Vec::new();
        let mut loaded_names = HashSet::new();
        for r in &rows {
            if r.kind == PAGE {
                if !r.url.is_empty() {
                    pages.insert(r.url.clone());
                }
                if !r.dupe && !r.hash.is_empty() {
                    page_hashes
                        .entry(r.hash.clone())
                        .or_insert_with(|| r.url.clone());
                }
            } else {
                if !r.url.is_empty() {
                    media_urls.insert(r.url.clone());
                }
                if !r.dupe {
                    if !r.hash.is_empty() {
                        loaded_hashes.push((r.hash.clone(), r.name.clone()));
                    }
                    loaded_names.insert(r.name.clone());
                }
            }
        }

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)
            .map_err(|e| err(&e))?;
        let len = file.metadata().map_err(|e| err(&e))?.len();
        if len == 0 {
            file.write_all(&encode(&HEADER).map_err(|e| err(&e))?)
                .map_err(|e| err(&e))?;
            file.sync_data().map_err(|e| err(&e))?;
        } else {
            // a run killed mid-write may have left a last line without newline
            let mut last = [0u8; 1];
            file.seek(SeekFrom::End(-1)).map_err(|e| err(&e))?;
            file.read_exact(&mut last).map_err(|e| err(&e))?;
            if last[0] != b'\n' {
                file.write_all(b"\n").map_err(|e| err(&e))?;
            }
        }

        Ok(CsvLog {
            file: Mutex::new(file),
            media_urls: Mutex::new(media_urls),
            pages: Mutex::new(pages),
            page_hashes: Mutex::new(page_hashes),
            loaded_hashes,
            loaded_names,
            loaded_rows: rows.len(),
            bad_rows: bad,
            migrated: legacy,
        })
    }

    pub fn has_media_url(&self, url: &str) -> bool {
        self.media_urls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(url)
    }

    pub fn has_page(&self, url: &str) -> bool {
        self.pages
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(url)
    }

    /// URL of the first (non-duplicate) page that had this content.
    pub fn page_hash_owner(&self, hash: &str) -> Option<String> {
        self.page_hashes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(hash)
            .cloned()
    }

    pub fn loaded_hashes(&self) -> &[(String, String)] {
        &self.loaded_hashes
    }

    pub fn has_loaded_name(&self, name: &str) -> bool {
        self.loaded_names.contains(name)
    }

    /// Append one row and make sure it is on disk. Fields containing commas, quotes or
    /// newlines are quoted/escaped by the csv writer.
    pub fn write_row(
        &self,
        kind: &str,
        url: &str,
        name: &str,
        hash: &str,
        dupe: bool,
    ) -> std::io::Result<()> {
        let buf = encode(&[kind, url, name, hash, if dupe { "1" } else { "0" }])?;

        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        f.write_all(&buf)?;
        f.flush()?;
        f.sync_data()?;
        drop(f);

        if kind == PAGE {
            if !url.is_empty() {
                self.pages
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(url.to_string());
            }
            if !dupe && !hash.is_empty() {
                self.page_hashes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .entry(hash.to_string())
                    .or_insert_with(|| url.to_string());
            }
        } else if !url.is_empty() {
            self.media_urls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(url.to_string());
        }
        Ok(())
    }
}
