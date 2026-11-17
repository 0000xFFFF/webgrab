use crate::ui::{K, Printer};
use rusqlite::{Connection, OpenFlags};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use url::Url;

struct Page {
    url: String,
    depth: Option<i64>,
}

struct Media {
    url: Option<String>,
    status: String,
    path: Option<String>,
}

/// One line still to print.
enum Entry {
    /// page index, prefix, is last sibling, host of the parent page (None for a root)
    Page(usize, String, bool, Option<String>),
    /// media index, prefix, is last sibling, host of its page
    File(usize, String, bool, Option<String>),
}

fn host_of(url: &str) -> Option<String> {
    let u = Url::parse(url).ok()?;
    let h = u.host_str()?;
    Some(h.strip_prefix("www.").unwrap_or(h).to_string())
}

/// `/path?query`, with the host in front when it differs from the parent's host.
fn short(url: &str, parent_host: Option<&str>) -> String {
    let Ok(u) = Url::parse(url) else {
        return url.to_string();
    };
    let mut s = u.path().to_string();
    if let Some(q) = u.query() {
        s.push('?');
        s.push_str(q);
    }
    match (host_of(url), parent_host) {
        (Some(h), Some(p)) if h == p => s,
        (Some(h), _) => format!("{}{}", h, s),
        _ => s,
    }
}

/// --tree: print what was downloaded as a tree. Roots are the pages the crawl started from; a
/// page's children are the pages it links to (each page appears once, under the page that
/// reached it first) and its leaves are the files. A file shown on many pages is listed once,
/// under the first. Branches without files are left out.
pub fn print(db: &Path, ui: &Printer) -> Result<(), String> {
    let err = |e: &dyn std::fmt::Display| format!("--tree {}: {}", db.display(), e);
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| err(&e))?;

    // ---- load
    let mut pages: Vec<Page> = Vec::new();
    let mut pidx: HashMap<i64, usize> = HashMap::new();
    {
        let mut st = conn
            .prepare("SELECT id, url, depth FROM pages ORDER BY id")
            .map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })
            .map_err(|e| err(&e))?;
        for r in rows {
            let (id, url, depth) = r.map_err(|e| err(&e))?;
            pidx.insert(id, pages.len());
            pages.push(Page { url, depth });
        }
    }
    let mut media: Vec<Media> = Vec::new();
    let mut midx: HashMap<i64, usize> = HashMap::new();
    {
        let mut st = conn
            .prepare("SELECT id, url, status, file_path FROM media ORDER BY id")
            .map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| err(&e))?;
        for r in rows {
            let (id, url, status, path) = r.map_err(|e| err(&e))?;
            midx.insert(id, media.len());
            media.push(Media { url, status, path });
        }
    }

    // page -> pages it links to or redirects to (sorted by id, so the output is stable)
    let mut out_edges: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    let mut has_incoming = vec![false; pages.len()];
    for sql in [
        "SELECT from_page, to_page FROM links ORDER BY from_page, to_page",
        "SELECT from_page, to_page FROM redirects ORDER BY from_page, to_page",
    ] {
        let mut st = conn.prepare(sql).map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| err(&e))?;
        for r in rows {
            let (a, b) = r.map_err(|e| err(&e))?;
            if let (Some(&a), Some(&b)) = (pidx.get(&a), pidx.get(&b)) {
                if a != b {
                    out_edges[a].push(b);
                    has_incoming[b] = true;
                }
            }
        }
    }
    let mut page_media: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    {
        let mut st = conn
            .prepare("SELECT page_id, media_id FROM page_media ORDER BY media_id")
            .map_err(|e| err(&e))?;
        let rows = st
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| err(&e))?;
        for r in rows {
            let (p, m) = r.map_err(|e| err(&e))?;
            if let (Some(&p), Some(&m)) = (pidx.get(&p), midx.get(&m)) {
                page_media[p].push(m);
            }
        }
    }

    // ---- spanning forest: every page gets one parent (the page that reached it first)
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    let mut seen = vec![false; pages.len()];
    let mut roots: Vec<usize> = Vec::new();
    let mut grow = |starts: Vec<usize>, roots: &mut Vec<usize>, seen: &mut Vec<bool>| {
        let mut q: VecDeque<usize> = VecDeque::new();
        for s in starts {
            if !seen[s] {
                seen[s] = true;
                roots.push(s);
                q.push_back(s);
            }
        }
        while let Some(p) = q.pop_front() {
            for &c in &out_edges[p] {
                if !seen[c] {
                    seen[c] = true;
                    children[p].push(c);
                    q.push_back(c);
                }
            }
        }
    };
    // the pages the crawl started from, then pages nothing links to, then anything left (cycles)
    grow(
        (0..pages.len()).filter(|&i| pages[i].depth == Some(0)).collect(),
        &mut roots,
        &mut seen,
    );
    grow(
        (0..pages.len()).filter(|&i| !has_incoming[i]).collect(),
        &mut roots,
        &mut seen,
    );
    for i in 0..pages.len() {
        grow(vec![i], &mut roots, &mut seen);
    }

    // ---- walk in print order; a file belongs to the first page that shows it
    let mut order: Vec<usize> = Vec::new();
    {
        let mut stack: Vec<usize> = roots.iter().rev().copied().collect();
        while let Some(p) = stack.pop() {
            order.push(p);
            stack.extend(children[p].iter().rev());
        }
    }
    let mut owner_done = vec![false; media.len()];
    let mut owned: Vec<Vec<usize>> = vec![Vec::new(); pages.len()];
    for &p in &order {
        for &m in &page_media[p] {
            if !owner_done[m] {
                owner_done[m] = true;
                owned[p].push(m);
            }
        }
    }
    // files that no page shows (direct links, --hash-existing)
    let loose: Vec<usize> = (0..media.len()).filter(|&m| !owner_done[m]).collect();

    // files / pages in each subtree (children come after their parent in `order`)
    let mut files = vec![0usize; pages.len()];
    let mut npages = vec![0usize; pages.len()];
    for &p in order.iter().rev() {
        files[p] = owned[p].len();
        if files[p] > 0 {
            npages[p] = 1;
        }
        for &c in &children[p] {
            files[p] += files[c];
            if files[c] > 0 {
                npages[p] += npages[c];
            }
        }
        if files[p] > 0 && npages[p] == 0 {
            npages[p] = 1;
        }
    }

    // ---- print
    let file_line = |m: usize, host: Option<&str>| -> (K, String) {
        let md = &media[m];
        let name = match (&md.url, &md.path) {
            (Some(u), _) => short(u, host),
            (None, Some(p)) => p.clone(),
            (None, None) => "(unknown)".to_string(),
        };
        match md.status.as_str() {
            "duplicate" => (K::War, format!("{}  ≡ duplicate", name)),
            "failed" => (K::Eri, format!("{}  ✗ failed", name)),
            "queued" => (K::War, format!("{}  … not downloaded", name)),
            _ => (K::Def, name),
        }
    };

    let mut printed = false;
    for &root in roots.iter().filter(|&&r| files[r] > 0) {
        if printed {
            ui.always(K::Def, "");
        }
        printed = true;
        let mut stack = vec![Entry::Page(root, String::new(), true, None)];
        while let Some(e) = stack.pop() {
            match e {
                Entry::File(m, prefix, last, host) => {
                    let (k, text) = file_line(m, host.as_deref());
                    let conn = if last { "└─ " } else { "├─ " };
                    ui.always(k, &format!("{}{}{}", prefix, conn, text));
                }
                Entry::Page(p, prefix, last, parent_host) => {
                    let is_root = parent_host.is_none() && prefix.is_empty() && p == root;
                    let own_host = host_of(&pages[p].url);
                    let child_prefix = if is_root {
                        String::new()
                    } else {
                        let conn = if last { "└─ " } else { "├─ " };
                        ui.always(
                            K::Inf,
                            &format!(
                                "{}{}▸ {}",
                                prefix,
                                conn,
                                short(&pages[p].url, parent_host.as_deref())
                            ),
                        );
                        format!("{}{}", prefix, if last { "   " } else { "│  " })
                    };
                    if is_root {
                        ui.always(
                            K::Top,
                            &format!(
                                "◆ {}  ({} page{}, {} file{})",
                                pages[p].url,
                                npages[p],
                                if npages[p] == 1 { "" } else { "s" },
                                files[p],
                                if files[p] == 1 { "" } else { "s" },
                            ),
                        );
                    }
                    // this page's files first, then the pages below it
                    let kids: Vec<usize> =
                        children[p].iter().copied().filter(|&c| files[c] > 0).collect();
                    let total = owned[p].len() + kids.len();
                    let mut entries: Vec<Entry> = Vec::with_capacity(total);
                    for (i, &m) in owned[p].iter().enumerate() {
                        entries.push(Entry::File(
                            m,
                            child_prefix.clone(),
                            i + 1 == total,
                            own_host.clone(),
                        ));
                    }
                    for (i, &c) in kids.iter().enumerate() {
                        entries.push(Entry::Page(
                            c,
                            child_prefix.clone(),
                            owned[p].len() + i + 1 == total,
                            own_host.clone(),
                        ));
                    }
                    stack.extend(entries.into_iter().rev());
                }
            }
        }
    }

    if !loose.is_empty() {
        if printed {
            ui.always(K::Def, "");
        }
        printed = true;
        ui.always(
            K::Top,
            &format!(
                "◆ (files not tied to a page)  ({} file{})",
                loose.len(),
                if loose.len() == 1 { "" } else { "s" }
            ),
        );
        for (i, &m) in loose.iter().enumerate() {
            let (k, text) = file_line(m, None);
            let conn = if i + 1 == loose.len() { "└─ " } else { "├─ " };
            ui.always(k, &format!("{}{}", conn, text));
        }
    }

    if !printed {
        ui.always(K::Inf, "(no files in this database yet)");
    }
    Ok(())
}
