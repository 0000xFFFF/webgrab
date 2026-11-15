use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

const SCHEMA_VERSION: i64 = 1;

/// The whole crawl as a graph, in one SQLite file.
///
///   pages        a web page: url, title, http status, hash of its html ...
///   media        a file: url, where it was saved, mime type, size, sha256, status ...
///   links        page  --links to-->   page
///   page_media   page  --has-->        media     (every media a page shows, even if it was
///                                                 downloaded while crawling another page)
///   redirects    page  --redirects to--> page
///
/// plus views for reading it without joins: v_page_media, v_links, v_redirects,
/// v_media_dupes and v_edges (all relationships in one list, handy for Gephi/graph tools).
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS pages (
    id             INTEGER PRIMARY KEY,
    url            TEXT    NOT NULL UNIQUE,
    title          TEXT,
    http_status    INTEGER,                 -- status of the last request (NULL = never requested)
    content_type   TEXT,
    size           INTEGER,                 -- bytes of html
    html_sha256    TEXT,
    is_dupe        INTEGER NOT NULL DEFAULT 0,  -- 1 = same html as another page (dupe_of)
    dupe_of        INTEGER REFERENCES pages(id),
    depth          INTEGER,                 -- crawl depth it was first reached at
    error          TEXT,                    -- last request error (NULL = ok)
    discovered_at  TEXT    NOT NULL DEFAULT ({NOW}),
    crawled_at     TEXT                     -- NULL = only seen as a link, never read
);
CREATE INDEX IF NOT EXISTS idx_pages_hash ON pages(html_sha256);

CREATE TABLE IF NOT EXISTS media (
    id             INTEGER PRIMARY KEY,
    url            TEXT UNIQUE,             -- NULL for files that were already in --dir (--hash-existing)
    kind           TEXT,                    -- image | video | file
    status         TEXT    NOT NULL DEFAULT 'queued',  -- queued | downloaded | duplicate | failed | existing
    file_path      TEXT,                    -- relative to --dir (NULL for duplicates / failures)
    mime_type      TEXT,
    size           INTEGER,
    sha256         TEXT,
    dupe_of        INTEGER REFERENCES media(id),  -- status=duplicate: the file that was kept
    http_status    INTEGER,
    error          TEXT,
    discovered_at  TEXT    NOT NULL DEFAULT ({NOW}),
    downloaded_at  TEXT
);
CREATE INDEX IF NOT EXISTS idx_media_hash ON media(sha256);
CREATE INDEX IF NOT EXISTS idx_media_path ON media(file_path);

CREATE TABLE IF NOT EXISTS links (
    from_page INTEGER NOT NULL REFERENCES pages(id),
    to_page   INTEGER NOT NULL REFERENCES pages(id),
    PRIMARY KEY (from_page, to_page)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_links_to ON links(to_page);

CREATE TABLE IF NOT EXISTS page_media (
    page_id  INTEGER NOT NULL REFERENCES pages(id),
    media_id INTEGER NOT NULL REFERENCES media(id),
    PRIMARY KEY (page_id, media_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_page_media_media ON page_media(media_id);

CREATE TABLE IF NOT EXISTS redirects (
    from_page INTEGER NOT NULL REFERENCES pages(id),
    to_page   INTEGER NOT NULL REFERENCES pages(id),
    PRIMARY KEY (from_page, to_page)
) WITHOUT ROWID;

CREATE VIEW IF NOT EXISTS v_page_media AS
    SELECT p.url AS page_url, p.title AS page_title, m.url AS media_url, m.kind,
           m.status, m.file_path, m.mime_type, m.size, m.sha256
    FROM page_media pm JOIN pages p ON p.id = pm.page_id JOIN media m ON m.id = pm.media_id;

CREATE VIEW IF NOT EXISTS v_links AS
    SELECT a.url AS from_url, a.title AS from_title, b.url AS to_url, b.title AS to_title
    FROM links l JOIN pages a ON a.id = l.from_page JOIN pages b ON b.id = l.to_page;

CREATE VIEW IF NOT EXISTS v_redirects AS
    SELECT a.url AS from_url, b.url AS to_url
    FROM redirects r JOIN pages a ON a.id = r.from_page JOIN pages b ON b.id = r.to_page;

CREATE VIEW IF NOT EXISTS v_media_dupes AS
    SELECT d.url AS duplicate_url, o.url AS original_url, o.file_path AS kept_file, d.sha256
    FROM media d JOIN media o ON o.id = d.dupe_of WHERE d.status = 'duplicate';

CREATE VIEW IF NOT EXISTS v_edges AS
    SELECT 'page' AS from_type, a.url AS from_url, 'page' AS to_type, b.url AS to_url, 'links_to' AS relation
        FROM links l JOIN pages a ON a.id = l.from_page JOIN pages b ON b.id = l.to_page
    UNION ALL
    SELECT 'page', p.url, 'media', COALESCE(m.url, m.file_path), 'has_media'
        FROM page_media pm JOIN pages p ON p.id = pm.page_id JOIN media m ON m.id = pm.media_id
    UNION ALL
    SELECT 'page', a.url, 'page', b.url, 'redirects_to'
        FROM redirects r JOIN pages a ON a.id = r.from_page JOIN pages b ON b.id = r.to_page;
"#;

const NOW: &str = "strftime('%Y-%m-%dT%H:%M:%SZ','now')";

/// A page that was read (fetched + scanned).
pub struct PageRec<'a> {
    pub url: &'a str,
    pub title: &'a str,
    pub status: Option<u16>,
    pub content_type: Option<&'a str>,
    pub size: usize,
    pub sha256: Option<&'a str>,
    pub depth: usize,
    /// url of the earlier page with the same html (this one is a duplicate)
    pub dupe_of: Option<&'a str>,
    /// the page was already in the db (it is only being read again to find its links)
    pub refresh: bool,
}

/// SQLite database shared by all threads (every call is one short transaction, so the file
/// is always consistent and a killed run loses nothing that was already reported).
pub struct Db {
    conn: Mutex<Connection>,
    pub pages_loaded: usize,
    pub media_loaded: usize,
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        let err = |e: &dyn std::fmt::Display| format!("--db {}: {}", path.display(), e);
        let conn = Connection::open(path).map_err(|e| err(&e))?;
        conn.busy_timeout(Duration::from_secs(10))
            .map_err(|e| err(&e))?;
        // WAL: fast, and the file can be opened by a viewer while the crawl is running
        conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
            .map_err(|e| err(&e))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| err(&e))?;

        let ver: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(|e| err(&e))?;
        if ver > SCHEMA_VERSION {
            return Err(err(&format!("made by a newer webgrab (schema v{})", ver)));
        }
        conn.execute_batch(&SCHEMA.replace("{NOW}", NOW))
            .map_err(|e| err(&e))?;
        if ver == 0 {
            conn.execute_batch(&format!("PRAGMA user_version={}", SCHEMA_VERSION))
                .map_err(|e| err(&e))?;
        }

        let count = |t: &str| -> usize {
            conn.query_row(&format!("SELECT COUNT(*) FROM {}", t), [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0) as usize
        };
        let (pages_loaded, media_loaded) = (count("pages"), count("media"));
        Ok(Db {
            conn: Mutex::new(conn),
            pages_loaded,
            media_loaded,
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>) -> Result<T, String> {
        let mut g = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g).map_err(|e| e.to_string())
    }

    fn exists(&self, sql: &str, arg: &str) -> bool {
        self.with(|c| {
            c.query_row(sql, [arg], |_| Ok(()))
                .optional()
                .map(|o| o.is_some())
        })
        .unwrap_or(false)
    }

    // ------------------------------------------------------------------ lookups

    /// Already handled by an earlier run (saved, or deleted as a duplicate)? Failed/queued ones are retried.
    pub fn media_done(&self, url: &str) -> bool {
        self.exists(
            "SELECT 1 FROM media WHERE url=?1 AND status IN ('downloaded','duplicate')",
            url,
        )
    }

    pub fn has_file_path(&self, path: &str) -> bool {
        self.exists(
            "SELECT 1 FROM media WHERE file_path=?1 AND status IN ('downloaded','existing')",
            path,
        )
    }

    /// (sha256, file path) of every file we have on disk - feeds --nodupes.
    pub fn file_hashes(&self) -> Vec<(String, String)> {
        self.with(|c| {
            let mut st = c.prepare(
                "SELECT sha256, file_path FROM media
                 WHERE sha256 IS NOT NULL AND file_path IS NOT NULL AND status IN ('downloaded','existing')",
            )?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect()
        })
        .unwrap_or_default()
    }

    /// Was the page read completely before?
    pub fn page_seen(&self, url: &str) -> bool {
        self.exists(
            "SELECT 1 FROM pages WHERE url=?1 AND crawled_at IS NOT NULL",
            url,
        )
    }

    /// url of the first page that had exactly this html
    pub fn page_hash_owner(&self, hash: &str) -> Option<String> {
        self.with(|c| {
            c.query_row(
                "SELECT url FROM pages WHERE html_sha256=?1 AND is_dupe=0 AND crawled_at IS NOT NULL
                 ORDER BY id LIMIT 1",
                [hash],
                |r| r.get::<_, String>(0),
            )
            .optional()
        })
        .unwrap_or(None)
    }

    // -------------------------------------------------------------------- pages

    /// A page was read.
    pub fn page_done(&self, p: &PageRec) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO pages(url,title,http_status,content_type,size,html_sha256,is_dupe,dupe_of,depth,crawled_at)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,(SELECT id FROM pages WHERE url=?8),?9,{NOW})
                     ON CONFLICT(url) DO UPDATE SET
                        title=excluded.title, http_status=excluded.http_status,
                        content_type=excluded.content_type, size=excluded.size,
                        html_sha256=excluded.html_sha256,
                        is_dupe = CASE WHEN ?10 THEN pages.is_dupe ELSE excluded.is_dupe END,
                        dupe_of = CASE WHEN ?10 THEN pages.dupe_of ELSE excluded.dupe_of END,
                        depth=COALESCE(pages.depth, excluded.depth),
                        error=NULL, crawled_at=excluded.crawled_at"
                ),
                params![
                    p.url,
                    p.title,
                    p.status,
                    p.content_type,
                    p.size as i64,
                    p.sha256,
                    p.dupe_of.is_some() as i64,
                    p.dupe_of,
                    p.depth as i64,
                    p.refresh
                ],
            )
            .map(|_| ())
        })
    }

    /// The request for a page failed (it stays "not crawled", so the next run tries again).
    pub fn page_failed(&self, url: &str, status: Option<u16>, error: &str) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO pages(url,http_status,error) VALUES(?1,?2,?3)
                 ON CONFLICT(url) DO UPDATE SET http_status=excluded.http_status, error=excluded.error",
                params![url, status, error],
            )
            .map(|_| ())
        })
    }

    /// `from` redirected to `to`: remember the edge and that `from` needs no more visits.
    pub fn redirect(&self, from: &str, to: &str) -> Result<(), String> {
        self.with(|c| {
            let tx = c.transaction()?;
            tx.execute("INSERT OR IGNORE INTO pages(url) VALUES(?1)", [to])?;
            tx.execute(
                &format!(
                    "INSERT INTO pages(url,crawled_at) VALUES(?1,{NOW})
                     ON CONFLICT(url) DO UPDATE SET crawled_at=COALESCE(pages.crawled_at, excluded.crawled_at)"
                ),
                [from],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO redirects(from_page,to_page)
                 SELECT a.id, b.id FROM pages a, pages b WHERE a.url=?1 AND b.url=?2",
                [from, to],
            )?;
            tx.commit()
        })
    }

    /// page --links to--> each of `to` (pages we haven't read yet are added as "seen only")
    pub fn link_pages(&self, page: &str, to: &[String]) -> Result<(), String> {
        self.with(|c| {
            let tx = c.transaction()?;
            tx.execute("INSERT OR IGNORE INTO pages(url) VALUES(?1)", [page])?;
            {
                let mut ins = tx.prepare_cached("INSERT OR IGNORE INTO pages(url) VALUES(?1)")?;
                let mut edge = tx.prepare_cached(
                    "INSERT OR IGNORE INTO links(from_page,to_page)
                     SELECT a.id, b.id FROM pages a, pages b WHERE a.url=?1 AND b.url=?2",
                )?;
                for t in to.iter().filter(|t| t.as_str() != page) {
                    ins.execute([t])?;
                    edge.execute([page, t.as_str()])?;
                }
            }
            tx.commit()
        })
    }

    // -------------------------------------------------------------------- media

    /// page --has--> each of `media` ((url, kind); files are added as "queued" if new)
    pub fn link_media(&self, page: &str, media: &[(String, &'static str)]) -> Result<(), String> {
        self.with(|c| {
            let tx = c.transaction()?;
            tx.execute("INSERT OR IGNORE INTO pages(url) VALUES(?1)", [page])?;
            {
                let mut ins = tx.prepare_cached(
                    "INSERT INTO media(url,kind) VALUES(?1,?2)
                     ON CONFLICT(url) DO UPDATE SET kind=COALESCE(media.kind, excluded.kind)",
                )?;
                let mut edge = tx.prepare_cached(
                    "INSERT OR IGNORE INTO page_media(page_id,media_id)
                     SELECT p.id, m.id FROM pages p, media m WHERE p.url=?1 AND m.url=?2",
                )?;
                for (url, kind) in media {
                    ins.execute(params![url, kind])?;
                    edge.execute(params![page, url])?;
                }
            }
            tx.commit()
        })
    }

    pub fn media_downloaded(
        &self,
        url: &str,
        kind: &str,
        path: &str,
        mime: Option<&str>,
        size: u64,
        sha: Option<&str>,
        http: u16,
    ) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO media(url,kind,status,file_path,mime_type,size,sha256,http_status,downloaded_at)
                     VALUES(?1,?2,'downloaded',?3,?4,?5,?6,?7,{NOW})
                     ON CONFLICT(url) DO UPDATE SET
                        kind=COALESCE(media.kind, excluded.kind), status='downloaded',
                        file_path=excluded.file_path, mime_type=excluded.mime_type, size=excluded.size,
                        sha256=excluded.sha256, http_status=excluded.http_status, error=NULL,
                        dupe_of=NULL, downloaded_at=excluded.downloaded_at"
                ),
                params![url, kind, path, mime, size as i64, sha, http],
            )
            .map(|_| ())
        })
    }

    /// The file had the same content as one we already have: it was deleted.
    pub fn media_duplicate(
        &self,
        url: &str,
        kind: &str,
        mime: Option<&str>,
        size: u64,
        sha: &str,
        http: u16,
    ) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO media(url,kind,status,file_path,mime_type,size,sha256,http_status,downloaded_at,dupe_of)
                     VALUES(?1,?2,'duplicate',NULL,?3,?4,?5,?6,{NOW},
                        (SELECT id FROM media WHERE sha256=?5 AND status IN ('downloaded','existing')
                         AND url IS NOT ?1 ORDER BY id LIMIT 1))
                     ON CONFLICT(url) DO UPDATE SET
                        kind=COALESCE(media.kind, excluded.kind), status='duplicate', file_path=NULL,
                        mime_type=excluded.mime_type, size=excluded.size, sha256=excluded.sha256,
                        http_status=excluded.http_status, error=NULL, dupe_of=excluded.dupe_of,
                        downloaded_at=excluded.downloaded_at"
                ),
                params![url, kind, mime, size as i64, sha, http],
            )
            .map(|_| ())
        })
    }

    pub fn media_failed(
        &self,
        url: &str,
        kind: &str,
        http: Option<u16>,
        error: &str,
    ) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                "INSERT INTO media(url,kind,status,http_status,error) VALUES(?1,?2,'failed',?3,?4)
                 ON CONFLICT(url) DO UPDATE SET
                    kind=COALESCE(media.kind, excluded.kind), status='failed',
                    http_status=excluded.http_status, error=excluded.error",
                params![url, kind, http, error],
            )
            .map(|_| ())
        })
    }

    /// --hash-existing: a file that was already in --dir (no url).
    pub fn media_existing(
        &self,
        path: &str,
        kind: &str,
        size: u64,
        sha: &str,
    ) -> Result<(), String> {
        self.with(|c| {
            c.execute(
                &format!(
                    "INSERT INTO media(url,kind,status,file_path,size,sha256,downloaded_at)
                     VALUES(NULL,?1,'existing',?2,?3,?4,{NOW})"
                ),
                params![kind, path, size as i64, sha],
            )
            .map(|_| ())
        })
    }
}

impl Drop for Db {
    /// Fold the write-ahead log back into the main file, so a finished run leaves one single file.
    fn drop(&mut self) {
        if let Ok(c) = self.conn.lock() {
            let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
    }
}
