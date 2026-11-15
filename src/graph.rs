use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::Path;

const TEMPLATE: &str = include_str!("graph.html");

/// --graph: turn the --db file into one standalone .html page with an interactive graph
/// (pages and files as nodes; links / has-media / redirects as edges).
/// Returns (pages, media, edges).
pub fn export(db: &Path, out: &Path) -> Result<(usize, usize, usize), String> {
    let err = |e: &dyn std::fmt::Display| format!("--graph {}: {}", db.display(), e);
    // read-write (never creates): closing it then also removes the -wal/-shm files
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_WRITE).map_err(|e| err(&e))?;

    let mut pages: Vec<Value> = Vec::new();
    {
        let mut st = conn
            .prepare(
                "SELECT id, url, title, http_status, depth, crawled_at IS NOT NULL, is_dupe, error, size, discovered_at
                 FROM pages ORDER BY id",
            )
            .map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "url": r.get::<_, String>(1)?,
                    "title": r.get::<_, Option<String>>(2)?,
                    "status": r.get::<_, Option<i64>>(3)?,
                    "depth": r.get::<_, Option<i64>>(4)?,
                    "crawled": r.get::<_, i64>(5)? != 0,
                    "dupe": r.get::<_, i64>(6)? != 0,
                    "error": r.get::<_, Option<String>>(7)?,
                    "size": r.get::<_, Option<i64>>(8)?,
                    "at": r.get::<_, Option<String>>(9)?,
                }))
            })
            .map_err(|e| err(&e))?;
        for r in rows {
            pages.push(r.map_err(|e| err(&e))?);
        }
    }

    let mut media: Vec<Value> = Vec::new();
    {
        let mut st = conn
            .prepare(
                "SELECT id, url, kind, status, file_path, mime_type, size, sha256, error, downloaded_at
                 FROM media ORDER BY id",
            )
            .map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "url": r.get::<_, Option<String>>(1)?,
                    "kind": r.get::<_, Option<String>>(2)?,
                    "status": r.get::<_, String>(3)?,
                    "path": r.get::<_, Option<String>>(4)?,
                    "mime": r.get::<_, Option<String>>(5)?,
                    "size": r.get::<_, Option<i64>>(6)?,
                    "sha256": r.get::<_, Option<String>>(7)?,
                    "error": r.get::<_, Option<String>>(8)?,
                    "at": r.get::<_, Option<String>>(9)?,
                }))
            })
            .map_err(|e| err(&e))?;
        for r in rows {
            media.push(r.map_err(|e| err(&e))?);
        }
    }

    // edges as [from, to, kind]: 0 = links to, 1 = has media, 2 = redirects to
    let mut edges: Vec<Value> = Vec::new();
    for (sql, kind) in [
        ("SELECT from_page, to_page FROM links", 0),
        ("SELECT page_id, media_id FROM page_media", 1),
        ("SELECT from_page, to_page FROM redirects", 2),
    ] {
        let mut st = conn.prepare(sql).map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| err(&e))?;
        for r in rows {
            let (a, b) = r.map_err(|e| err(&e))?;
            edges.push(json!([a, b, kind]));
        }
    }

    let data = json!({
        "db": db.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        "pages": pages,
        "media": media,
        "edges": edges,
    });
    // "</script>" inside a title must not end the script block
    let json_text = serde_json::to_string(&data).map_err(|e| err(&e))?.replace("</", "<\\/");
    let html = TEMPLATE.replace("__GRAPH_DATA__", &json_text);
    std::fs::write(out, html).map_err(|e| format!("--graph: can't write {}: {}", out.display(), e))?;

    Ok((pages.len(), media.len(), edges.len()))
}
