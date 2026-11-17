mod cli;
mod db;
mod download;
mod filter;
mod graph;
mod scan;
mod skip;
mod stats;
mod tree;
mod ua;
mod ui;
mod util;

use clap::Parser;
use cli::Cli;
use db::PageRec;
use download::{Ctx, sha256_hex};
use filter::Hint;
use stats::Stats;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};
use ui::{K, Printer};
use url::Url;
use util::{file_name_from_url, human, strip_fragment, url_ext};

fn main() -> ExitCode {
    #[cfg(windows)]
    {
        let _ = enable_ansi_support::enable_ansi_support();
    }
    let mut cli = Cli::parse_from(cli::normalized_args());
    cli.resolve();
    ExitCode::from(run(&cli))
}

fn run(cli: &Cli) -> u8 {
    let ui = Printer::new(cli.colorless, cli.compact);
    let stats = Stats::default();

    // --db crawl.db --graph crawl.html / --tree crawl.db (nothing to download): read an earlier crawl
    if !(cli.watch || cli.read.is_some() || cli.input.is_some()) {
        if cli.tree.is_none() && cli.graph.is_none() {
            ui.always(
                K::Cer,
                "no input: give a URL, an .html file, --read <file>, --watch or --tree <file.db>",
            );
            return 1;
        }
        if let Some(db) = &cli.tree {
            let t = print_tree(&ui, db);
            if t != 0 {
                return t;
            }
        }
        return match &cli.graph {
            Some(out) => export_graph(cli, &ui, out),
            None => 0,
        };
    }

    let ctx = match Ctx::new(cli, &ui, &stats) {
        Ok(c) => c,
        Err(e) => {
            ui.always(K::Cer, &e);
            return 1;
        }
    };

    let code = if cli.watch {
        watch(&ctx)
    } else if let Some(p) = &cli.read {
        read_mode(&ctx, p)
    } else if let Some(input) = &cli.input {
        page_mode(&ctx, input)
    } else {
        1
    };
    drop(ctx); // closes the database (folds its log into the single file)

    if let Some(db) = &cli.tree {
        let t = print_tree(&ui, db);
        if t != 0 {
            return t;
        }
    }
    if let Some(out) = &cli.graph {
        let g = export_graph(cli, &ui, out);
        if g != 0 {
            return g;
        }
    }
    code
}

fn print_tree(ui: &Printer, db: &Path) -> u8 {
    if !db.is_file() {
        ui.always(
            K::Cer,
            &format!("--tree: database not found: {}", db.display()),
        );
        return 1;
    }
    match tree::print(db, ui) {
        Ok(()) => 0,
        Err(e) => {
            ui.always(K::Cer, &e);
            1
        }
    }
}

fn export_graph(cli: &Cli, ui: &Printer, out: &Path) -> u8 {
    let Some(db) = &cli.db else {
        ui.always(
            K::Cer,
            "--graph needs --db <file.db> (the database to draw)",
        );
        return 1;
    };
    if !db.is_file() {
        ui.always(
            K::Cer,
            &format!("--graph: database not found: {}", db.display()),
        );
        return 1;
    }
    match graph::export(db, out) {
        Ok((p, m, e)) => {
            ui.always(
                K::Pri,
                &format!(
                    "graph: {} page(s), {} file(s), {} relationship(s) -> {}",
                    p,
                    m,
                    e,
                    out.display()
                ),
            );
            0
        }
        Err(e) => {
            ui.always(K::Cer, &e);
            1
        }
    }
}

// ------------------------------------------------------------------ page

fn is_direct_file(input: &str) -> bool {
    use filter::{TYPES_IMAGE, TYPES_OTHER, TYPES_VIDEO};
    match url_ext(input) {
        Some(e) => [TYPES_VIDEO, TYPES_IMAGE, TYPES_OTHER]
            .iter()
            .any(|l| l.contains(&e.as_str())),
        None => false,
    }
}

/// A page that was fetched (or read from a local file).
struct Fetched {
    html: Vec<u8>,
    /// where relative links are resolved against (after redirects; --base for a local file)
    base: Option<Url>,
    status: Option<u16>,
    content_type: Option<String>,
}

struct FetchErr {
    status: Option<u16>,
    msg: String,
}

fn page_mode(ctx: &Ctx, input: &str) -> u8 {
    let (cli, ui, stats) = (ctx.cli, ctx.ui, ctx.stats);
    let dl_mode = !cli.list && !cli.test;
    let is_file = Path::new(input).is_file();

    if !is_file && !filter::is_http(input) {
        ui.always(K::Cer, &format!("not an url or file: {}", input));
        return 1;
    }

    // direct link to a media file: download it (or just list it) and stop
    if !is_file && is_direct_file(input) {
        if cli.list {
            ui.plain(input);
            return 0;
        }
        if dl_mode {
            stats.total.store(1, Relaxed);
            stats.accepted.store(1, Relaxed);
            return finish_downloads(ctx, &[input.to_string()]);
        }
    }

    let base_arg: Option<Url> = cli.base.as_deref().and_then(|b| Url::parse(b).ok());
    if cli.base.is_some() && base_arg.is_none() {
        ui.always(K::Cer, "--base is not a valid URL");
        return 1;
    }

    let start_url: String;
    let fetched: Fetched;
    if is_file {
        let html = match std::fs::read(input) {
            Ok(b) => b,
            Err(e) => {
                ui.always(K::Cer, &format!("ERR[IO] {}: {}", input, e));
                return 1;
            }
        };
        // the db identifies a local page by its file:// url
        start_url = std::fs::canonicalize(input)
            .ok()
            .and_then(|p| Url::from_file_path(p).ok())
            .map(|u| u.to_string())
            .unwrap_or_else(|| input.to_string());
        fetched = Fetched {
            html,
            base: base_arg,
            status: None,
            content_type: Some("text/html".into()),
        };
    } else {
        if dl_mode {
            ui.say(K::Def, "");
            ui.say(K::Top, "===[ DOWNLOADING HTML ]");
            ui.say(K::Cur, &format!("DING..: {}", input));
        }
        match ctx.get(input).call() {
            Ok(resp) => {
                let status = resp.status();
                let content_type = resp.header("content-type").map(|c| c.to_string());
                let final_url = resp.get_url().to_string(); // after redirects
                let mut buf = Vec::new();
                if let Err(e) = resp.into_reader().take(128 << 20).read_to_end(&mut buf) {
                    ui.always(K::Cer, &format!("ERR[IO] {}", e));
                    return 1;
                }
                start_url = strip_fragment(&final_url).to_string();
                if start_url != strip_fragment(input) {
                    ctx.db_redirect(strip_fragment(input), &start_url);
                }
                fetched = Fetched {
                    html: buf,
                    base: Url::parse(&final_url).ok(),
                    status: Some(status),
                    content_type,
                };
            }
            Err(ureq::Error::Status(code, _)) => {
                ctx.db_page_failed(strip_fragment(input), Some(code), &format!("HTTP {}", code));
                ui.always(K::Cer, &format!("ERR[HTTP {}] {}", code, input));
                return 1;
            }
            Err(e) => {
                ctx.db_page_failed(strip_fragment(input), None, &e.to_string());
                ui.always(K::Cer, &format!("ERR[{}] {}", e, input));
                return 1;
            }
        }
    }

    let title = scan::title(&String::from_utf8_lossy(&fetched.html));
    let len = fetched.html.len();
    if cli.test {
        ui.always(K::Top, "===[ TESTING WEBSITE ]");
        ui.always(
            K::Pri,
            &format!("html size (DLED)..: {} ({} bytes)", human(len as f64), len),
        );
        ui.always(K::Inf, &format!("title from html...: {}", title));
    } else if dl_mode {
        ui.say(
            K::Pri,
            &format!("DLED..: {} ({} bytes)", human(len as f64), len),
        );
        ui.say(K::Def, "");
        ui.say(K::Top, "===[ SCANNING HTML ]");
        ui.say(K::Inf, &format!("title from html.: {}", title));
    }

    crawl(ctx, start_url, fetched)
}

// ------------------------------------------------------------------ crawl (--crawl-depth)

fn host_key(u: &Url) -> Option<String> {
    u.host_str()
        .map(|h| h.trim_start_matches("www.").to_lowercase())
}

/// What one page contains.
struct Scan {
    /// files to download/list (filters passed, --unique applied)
    files: Vec<String>,
    /// every file the page shows, even those --unique dropped (for the db's page -> file edges)
    media: Vec<(String, Option<Hint>)>,
    /// pages it links to
    pages: Vec<String>,
}

/// Split one page into files and pages.
/// Pages = <a href>/frame links that look like web pages. Files = every other link that
/// passes the filters; links inside <img>/<video>/<audio>/<source> count as media even
/// without a file extension. Pages are not downloaded unless --codec is given.
fn scan_page(ctx: &Ctx, base: Option<&Url>, html: &[u8], seen: &mut HashSet<String>) -> Scan {
    let cli = ctx.cli;
    let text = String::from_utf8_lossy(html);
    let found = scan::extract(base, &text);
    let mut pages = scan::extract_pages(base, &text);
    ctx.stats.total.fetch_add(found.len(), Relaxed);

    // an element-declared media URL is never a page, even if it is also linked with <a href>
    let mut hints: HashMap<String, Hint> = HashMap::new();
    for f in &found {
        if let Some(h) = f.hint {
            hints.entry(strip_fragment(&f.url).to_string()).or_insert(h);
        }
    }
    pages.retain(|p| !hints.contains_key(p));
    let page_set: HashSet<&str> = pages.iter().map(|p| p.as_str()).collect();

    let mut files = Vec::new();
    let mut media = Vec::new();
    for f in found {
        let l = strip_fragment(&f.url).to_string(); // "a.png#x" is the same file as "a.png"
        if !cli.codec && page_set.contains(l.as_str()) {
            continue; // crawlable, not downloadable
        }
        let hint = hints.get(&l).copied();
        if !ctx.filters.passes(&l, hint) {
            ctx.stats.disposed.fetch_add(1, Relaxed);
            continue;
        }
        media.push((l.clone(), hint));
        if cli.unique && !seen.insert(l.clone()) {
            ctx.stats.disposed.fetch_add(1, Relaxed);
            continue;
        }
        ctx.stats.accepted.fetch_add(1, Relaxed);
        files.push(l);
    }
    Scan {
        files,
        media,
        pages,
    }
}

/// Pages from `pages` that should be crawled next: same host (unless --external), passing the
/// --crawl-any/--crawl-must/--crawl-ignore filters, never visited before. Marks them visited.
fn follow_pages(
    ctx: &Ctx,
    pages: Vec<String>,
    host: &Option<String>,
    visited: &mut HashSet<String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for p in pages {
        if !ctx.crawl_filters.passes(&p) {
            continue;
        }
        let Ok(u) = Url::parse(&p) else { continue };
        if !ctx.cli.external && host_key(&u) != *host {
            continue;
        }
        if visited.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

/// Random number in 0..max (std only: every RandomState has its own random keys).
fn rand_below(max: u64) -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    if max == 0 {
        return 0;
    }
    RandomState::new().build_hasher().finish() % max
}

/// The pause before the next page request: the larger of --crawl-sleep (the minimum) and the
/// sleep calibrated from earlier 429s, plus up to 10% random so it isn't a fixed rhythm.
fn page_pause(ctx: &Ctx) -> u64 {
    let mut p = ctx.pace.lock().unwrap_or_else(|e| e.into_inner());
    let base = ctx.cli.crawl_sleep.max(p.learned_ms);
    let pause = base + rand_below(base / 10 + 1);
    p.last_pause_ms = pause;
    pause
}

/// A 429 arrived. `used` = the pause we had left before this request, `took` = how long the
/// failed request itself took. Raise the calibrated sleep above what was evidently too fast:
/// at least 1.5x the old value, 2x the pause that failed, 2x the answer time, and 500 ms.
fn learn_from_429(ctx: &Ctx, url: &str, used: u64, took: Duration) {
    let (old, new) = {
        let mut p = ctx.pace.lock().unwrap_or_else(|e| e.into_inner());
        let old = p.learned_ms;
        let took_ms = took.as_millis() as u64;
        let new = (old + old / 2)
            .max(used.saturating_mul(2))
            .max(took_ms.saturating_mul(2))
            .max(500)
            .min(60_000);
        p.learned_ms = new;
        p.hits += 1;
        (old, new)
    };
    if !ctx.cli.quiet {
        let k = if ctx.cli.list || ctx.cli.test { K::Err } else { K::War };
        ctx.ui.say(
            k,
            &format!(
                "WAR[429] {} - failed request took {} ms after a {:.1}s pause; crawl sleep calibrated {:.1}s -> {:.1}s",
                url,
                took.as_millis(),
                used as f64 / 1000.0,
                old as f64 / 1000.0,
                new as f64 / 1000.0
            ),
        );
    }
}

/// GET a page. Waits the (calibrated) pause first. HTTP 429 is retried with exponential backoff
/// (1s, 2s, 4s ... max 60s, or the server's Retry-After if longer) plus a random offset, and every
/// 429 teaches the crawler a longer pause for the pages after it.
fn get_page(ctx: &Ctx, url: &str) -> Result<ureq::Response, FetchErr> {
    let cli = ctx.cli;
    let mut attempt: u32 = 0;
    let mut pause = page_pause(ctx);
    loop {
        if pause > 0 {
            std::thread::sleep(Duration::from_millis(pause));
        }
        let started = Instant::now();
        match ctx.get(url).call() {
            Ok(r) => return Ok(r),
            Err(ureq::Error::Status(429, resp)) => {
                learn_from_429(ctx, url, pause, started.elapsed());
                if cli.crawl_no_retry_429 || attempt >= cli.crawl_retries {
                    return Err(FetchErr {
                        status: Some(429),
                        msg: "HTTP 429".into(),
                    });
                }
                let backoff = (1000u64 << attempt.min(6)).min(60_000);
                let asked = resp
                    .header("retry-after")
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(|s| s.saturating_mul(1000).min(120_000))
                    .unwrap_or(0);
                // never wait less than the newly calibrated pause
                let learned = ctx.pace.lock().unwrap_or_else(|e| e.into_inner()).learned_ms;
                let base = backoff.max(asked).max(learned);
                pause = base + rand_below(base / 5 + 250); // + up to 20% and 250 ms
                attempt += 1;
                if !cli.quiet {
                    let k = if cli.list || cli.test { K::Err } else { K::War };
                    ctx.ui.say(
                        k,
                        &format!(
                            "WAR[429] {} - retry {}/{} in {:.1}s",
                            url,
                            attempt,
                            cli.crawl_retries,
                            pause as f64 / 1000.0
                        ),
                    );
                }
            }
            Err(ureq::Error::Status(code, _)) => {
                return Err(FetchErr {
                    status: Some(code),
                    msg: format!("HTTP {}", code),
                });
            }
            Err(e) => {
                return Err(FetchErr {
                    status: None,
                    msg: e.to_string(),
                });
            }
        }
    }
}

/// Ok(None) = the response is not HTML (not crawled).
fn fetch_page(ctx: &Ctx, url: &str) -> Result<Option<Fetched>, FetchErr> {
    let resp = get_page(ctx, url)?;
    let content_type = resp.header("content-type").map(|c| c.to_string());
    if let Some(ct) = &content_type {
        if !ct.to_lowercase().contains("html") {
            return Ok(None);
        }
    }
    let status = resp.status();
    let base = Url::parse(resp.get_url()).ok(); // after redirects
    let mut buf = Vec::new();
    resp.into_reader()
        .take(32 << 20)
        .read_to_end(&mut buf)
        .map_err(|e| FetchErr {
            status: Some(status),
            msg: e.to_string(),
        })?;
    Ok(Some(Fetched {
        html: buf,
        base,
        status: Some(status),
        content_type,
    }))
}

/// Progress lines. In --list/--test mode they go to stderr so stdout stays a clean link list.
fn crawl_log(ctx: &Ctx, text: &str) {
    if ctx.cli.list || ctx.cli.test {
        if !ctx.cli.quiet {
            ctx.ui.say(K::Err, text);
        }
    } else {
        ctx.ui.say(K::Def, "");
        ctx.ui.say(K::Top, text);
    }
}

/// List or download one page's files.
fn emit(ctx: &Ctx, files: &[String], collected: &mut Vec<String>, listed: &mut usize) {
    let cli = ctx.cli;
    if cli.list {
        if cli.verbose {
            collected.extend_from_slice(files); // table is printed once, at the end
        } else {
            print_list(ctx, files, *listed);
            *listed += files.len();
        }
    } else if !cli.test && !files.is_empty() {
        run_downloads(ctx, files);
    }
}

struct Crawl {
    host: Option<String>,
    visited: HashSet<String>,
    seen_files: HashSet<String>,
    collected: Vec<String>,
    listed: usize,
}

/// Process one fetched page: hash it, record it and its links in the --db, list/download its
/// files, and return the pages worth crawling next (empty at the last depth).
fn handle_page(ctx: &Ctx, st: &mut Crawl, d: usize, url: &str, f: &Fetched) -> Vec<String> {
    let known = ctx.page_seen(url); // an earlier run already read it (we only read it again for its links)
    let title = scan::title(&String::from_utf8_lossy(&f.html));
    let hash = ctx.tracks_pages().then(|| sha256_hex(&f.html));
    let mut rec = PageRec {
        url,
        title: &title,
        status: f.status,
        content_type: f.content_type.as_deref(),
        size: f.html.len(),
        sha256: hash.as_deref(),
        depth: d,
        dupe_of: None,
        refresh: known,
    };

    // same content as a page we already crawled under another URL: nothing new in it
    if !known {
        if let Some(h) = &hash {
            if let Some(owner) = ctx.page_hash_owner(h) {
                if owner != url {
                    ctx.stats.page_dupes.fetch_add(1, Relaxed);
                    rec.dupe_of = Some(&owner);
                    ctx.db_page_done(&rec);
                    if !ctx.cli.quiet {
                        ctx.ui.say(
                            K::War,
                            &format!("WAR[DUPLICATE PAGE] {} (same content as {})", url, owner),
                        );
                    }
                    return Vec::new();
                }
            }
        }
    }

    let scan = scan_page(ctx, f.base.as_ref(), &f.html, &mut st.seen_files);
    ctx.db_link_pages(url, &scan.pages);
    ctx.db_link_media(url, &scan.media);
    emit(ctx, &scan.files, &mut st.collected, &mut st.listed);
    // marked as read only after its files are done, so an interrupted page is read again next time
    ctx.db_page_done(&rec);
    ctx.stats.pages.fetch_add(1, Relaxed);

    if d < ctx.cli.crawl_depth {
        follow_pages(ctx, scan.pages, &st.host, &mut st.visited)
    } else {
        Vec::new()
    }
}

/// --crawl-depth N: breadth-first crawl (0 = just the start page). Files of every page are
/// downloaded (or listed) as soon as the page has been scanned, so an interrupted run (with --db)
/// can be resumed.
///
/// With --db every page is recorded. On a re-run:
///  - pages at the last depth that are in the db are skipped (nothing to find in them),
///  - pages above it are read again, only to find the pages below them (their files are skipped
///    because the db has them) - otherwise an interrupted crawl could never continue.
fn crawl(ctx: &Ctx, start_url: String, start: Fetched) -> u8 {
    let (cli, ui, stats) = (ctx.cli, ctx.ui, ctx.stats);
    let max = cli.crawl_depth;

    let start_key = strip_fragment(&start_url).to_string();
    let mut st = Crawl {
        host: start.base.as_ref().and_then(host_key),
        visited: HashSet::new(),
        seen_files: HashSet::new(),
        collected: Vec::new(),
        listed: 0,
    };
    st.visited.insert(start_key.clone());

    if start.base.is_none() {
        let k = if cli.list || cli.test { K::Err } else { K::War };
        ui.say(
            k,
            "WAR[--base] local file without --base: only absolute http(s) links can be used",
        );
    }

    // depth 0: the page we already have
    crawl_log(ctx, &format!("===[ CRAWL depth 0/{} ]: {}", max, start_key));
    let mut level = handle_page(ctx, &mut st, 0, &start_key, &start);
    drop(start);

    for d in 1..=max {
        if level.is_empty() {
            break;
        }
        let mut next: Vec<String> = Vec::new();
        let n = level.len();
        for (i, page) in level.iter().enumerate() {
            if d == max && ctx.page_seen(page) {
                stats.skipped.fetch_add(1, Relaxed);
                if !cli.quiet {
                    ui.say(
                        K::War,
                        &format!("WAR[SKIPPED] {} (page already in the db)", page),
                    );
                }
                continue;
            }
            crawl_log(
                ctx,
                &format!(
                    "===[ CRAWL depth {}/{} page {}/{} ]: {}",
                    d,
                    max,
                    i + 1,
                    n,
                    page
                ),
            );
            match fetch_page(ctx, page) {
                Ok(Some(f)) => {
                    let mut key = page.clone();
                    if let Some(b) = &f.base {
                        // redirected: the content belongs to the page we ended up on
                        let fin = strip_fragment(b.as_str());
                        if fin != page.as_str() {
                            ctx.db_redirect(page, fin);
                            if !st.visited.insert(fin.to_string()) {
                                continue; // ... which we've already seen
                            }
                            key = fin.to_string();
                        }
                    }
                    next.extend(handle_page(ctx, &mut st, d, &key, &f));
                }
                Ok(None) => {
                    if cli.verbose {
                        // stderr in --list/--test so stdout stays a clean link list
                        let k = if cli.list || cli.test { K::Err } else { K::War };
                        ui.say(k, &format!("WAR[NOT HTML] {}", page));
                    }
                }
                Err(e) => {
                    stats.failed.fetch_add(1, Relaxed);
                    ctx.db_page_failed(page, e.status, &e.msg);
                    if !cli.quiet {
                        ui.say(K::Err, &format!("ERR[{}] {}", e.msg, page));
                    }
                }
            }
        }
        level = next;
    }

    if cli.list {
        if cli.verbose {
            print_list(ctx, &st.collected, 0);
        }
        0
    } else if cli.test {
        print_test_counts(ctx);
        0
    } else {
        ui.say(K::Def, "");
        download_summary(ctx)
    }
}

// ------------------------------------------------------------- --read file

fn read_mode(ctx: &Ctx, path: &Path) -> u8 {
    let (cli, ui, stats) = (ctx.cli, ctx.ui, ctx.stats);
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            ui.always(K::Cer, &format!("ERR[IO] {}: {}", path.display(), e));
            return 1;
        }
    };
    if cli.crawl_depth > 0 {
        ui.say(
            K::War,
            "WAR[--crawl-depth] is ignored with --read (every line is downloaded as a file)",
        );
    }
    ui.say(K::Def, "");
    ui.say(K::Top, "===[ SCANNING TEXT FILE ]");
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect();
    stats.total.store(lines.len(), Relaxed);
    if !cli.list && !cli.test {
        ui.say(K::Inf, &format!("total items....: {}", lines.len()));
    }
    let lines = unique(ctx, lines);
    let links: Vec<String> = lines.into_iter().filter(|l| ctx.accept(l, None)).collect();
    if !cli.list && !cli.test {
        ui.say(K::Inf, &format!("accepted items.: {}", links.len()));
    }
    finish(ctx, links)
}

// ------------------------------------------------------------------ watch

#[cfg(feature = "clipboard")]
fn watch(ctx: &Ctx) -> u8 {
    let ui = ctx.ui;
    let mut cb = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            ui.always(K::Cer, &format!("ERR[clipboard] {}", e));
            return 1;
        }
    };
    ui.say(K::Def, "");
    ui.say(K::Top, "===[ DOWNLOADING COPIED URLS ]  (Ctrl+C to stop)");

    // whatever is on the clipboard right now is not "newly copied"
    let mut past = cb.get_text().unwrap_or_default();
    let mut seen: HashSet<String> = HashSet::new();
    loop {
        std::thread::sleep(Duration::from_millis(100));
        let clip = match cb.get_text() {
            Ok(c) => c,
            Err(_) => continue,
        };
        if clip == past {
            continue;
        }
        past = clip.clone();
        let clip = clip.trim();
        if clip.is_empty() {
            continue;
        }
        if ctx.cli.unique && !seen.insert(clip.to_string()) {
            ctx.stats.disposed.fetch_add(1, Relaxed);
            continue;
        }
        if ctx.accept(clip, None) {
            ctx.stats.queued.fetch_add(1, Relaxed);
            let _ = ctx.download(clip);
        }
    }
}

#[cfg(not(feature = "clipboard"))]
fn watch(ctx: &Ctx) -> u8 {
    ctx.ui.always(
        K::Cer,
        "this build has no clipboard support (rebuild with the `clipboard` feature)",
    );
    1
}

// ----------------------------------------------------------- shared steps

/// --unique: keep only the first occurrence of every URL.
fn unique(ctx: &Ctx, items: Vec<String>) -> Vec<String> {
    if !ctx.cli.unique {
        return items;
    }
    let before = items.len();
    let mut seen = HashSet::new();
    let out: Vec<String> = items
        .into_iter()
        .filter(|u| seen.insert(u.clone()))
        .collect();
    ctx.stats.disposed.fetch_add(before - out.len(), Relaxed);
    out
}

fn finish(ctx: &Ctx, links: Vec<String>) -> u8 {
    if ctx.cli.list {
        print_list(ctx, &links, 0);
        0
    } else if ctx.cli.test {
        print_test_counts(ctx);
        0
    } else {
        finish_downloads(ctx, &links)
    }
}

fn print_test_counts(ctx: &Ctx) {
    let s = ctx.stats;
    ctx.ui.always(
        K::Inf,
        &format!("total items.......: {}", s.total.load(Relaxed)),
    );
    ctx.ui.always(
        K::Inf,
        &format!("accepted items....: {}", s.accepted.load(Relaxed)),
    );
    ctx.ui.always(
        K::War,
        &format!("skipped/found.....: {}", s.skipped.load(Relaxed)),
    );
    ctx.ui.always(
        K::Eri,
        &format!("disposed/rejected.: {}", s.disposed.load(Relaxed)),
    );
}

/// `start` = how many links were already printed (keeps the numbering going across pages).
/// Links are numbered when printing to a terminal; piped/redirected output is one bare URL per line.
fn print_list(ctx: &Ctx, links: &[String], start: usize) {
    let cli = ctx.cli;
    let ui = ctx.ui;
    if cli.verbose {
        let w_num = links.len().to_string().len().max(3);
        let w_url = links.iter().map(|l| l.len()).max().unwrap_or(0).max(3);
        ui.plain(&format!(
            "{:<w_num$} {:<w_url$} {}",
            "NUM", "URL", "FILENAME"
        ));
        for (i, l) in links.iter().enumerate() {
            let n = file_name_from_url(l).unwrap_or_default();
            ui.plain(&format!("{:<w_num$} {:<w_url$} {}", i + 1, l, n));
        }
        ctx.stats.processed.store(links.len(), Relaxed);
        print_debug(ctx);
    } else if cli.filename {
        for l in links {
            if let Some(n) = file_name_from_url(l) {
                ui.plain(&n);
            }
        }
    } else if ui::stdout_is_tty() {
        for (i, l) in links.iter().enumerate() {
            ui.plain(&format!("[{}] {}", start + i + 1, l));
        }
    } else {
        for l in links {
            ui.plain(l);
        }
    }
}

fn finish_downloads(ctx: &Ctx, urls: &[String]) -> u8 {
    ctx.ui.say(K::Def, "");
    ctx.ui.say(K::Top, "===[ DOWNLOADING ]");
    run_downloads(ctx, urls);
    download_summary(ctx)
}

fn download_summary(ctx: &Ctx) -> u8 {
    if ctx.cli.compact {
        ctx.ui.ticker(&compact_line(ctx.stats));
        ctx.ui.always(K::Def, "");
    } else {
        print_debug(ctx);
    }
    if ctx.stats.failed.load(Relaxed) > 0 {
        2
    } else {
        0
    }
}

/// Worker pool: `--threads` threads pull URLs from a shared index.
fn run_downloads(ctx: &Ctx, urls: &[String]) {
    ctx.stats.queued.fetch_add(urls.len(), Relaxed);
    let threads = ctx.cli.threads.min(urls.len()).max(1);
    let next = AtomicUsize::new(0);
    let done = AtomicBool::new(false);

    std::thread::scope(|outer| {
        if ctx.cli.compact {
            outer.spawn(|| {
                while !done.load(Relaxed) {
                    ctx.ui.ticker(&compact_line(ctx.stats));
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
        }
        std::thread::scope(|inner| {
            for _ in 0..threads {
                inner.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        if i >= urls.len() {
                            break;
                        }
                        let _ = ctx.download(&urls[i]);
                    }
                });
            }
        });
        done.store(true, Relaxed);
    });
}

fn compact_line(s: &Stats) -> String {
    let g = |a: &AtomicUsize| a.load(Relaxed);
    let mut parts = Vec::new();
    let mut add = |label: &str, v: usize| {
        if v != 0 {
            parts.push(format!("{}: {}", label, v));
        }
    };
    add("total", g(&s.total));
    add("acptd", g(&s.accepted));
    add("count", g(&s.processed));
    add("dled", g(&s.dled));
    add("renamed", g(&s.renamed));
    add("replc", g(&s.replaced));
    add("hdupe", g(&s.hash_dupes));
    add("pages", g(&s.pages));
    add("pdupe", g(&s.page_dupes));
    add("skip", g(&s.skipped));
    add("junk", g(&s.disposed));
    add("fail", g(&s.failed));
    parts.join(" ")
}

fn print_debug(ctx: &Ctx) {
    let (ui, s) = (ctx.ui, ctx.stats);
    let g = |a: &AtomicUsize| a.load(Relaxed);
    let bytes = s.bytes.load(Relaxed);
    ui.always(K::Def, "");
    ui.always(K::Top, "===[ DEBUG ]");
    ui.always(K::Inf, &format!("total/found/all.....: {}", g(&s.total)));
    ui.always(K::Inf, &format!("accepted/valid......: {}", g(&s.accepted)));
    ui.always(
        K::Cur,
        &format!("count/processed.....: {}", g(&s.processed)),
    );
    ui.always(
        K::Pri,
        &format!(
            "dled/downloaded.....: {} ({} ({} bytes))",
            g(&s.dled),
            human(bytes as f64),
            bytes
        ),
    );
    ui.always(K::War, &format!("renamed.............: {}", g(&s.renamed)));
    ui.always(K::War, &format!("replaced............: {}", g(&s.replaced)));
    ui.always(
        K::War,
        &format!("same hash (deleted).: {}", g(&s.hash_dupes)),
    );
    ui.always(K::War, &format!("skipped/found.......: {}", g(&s.skipped)));
    if g(&s.pages) > 0 {
        ui.always(
            K::Cur,
            &format!(
                "pages crawled.......: {} ({} duplicate)",
                g(&s.pages),
                g(&s.page_dupes)
            ),
        );
    }
    ui.always(K::Eri, &format!("disposed/rejected...: {}", g(&s.disposed)));
    ui.always(K::Eri, &format!("failed..............: {}", g(&s.failed)));
}
