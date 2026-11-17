# webgrab (Rust)

Web page file scanner / downloader.

Build: `cargo build --release` -> `target/release/webgrab` (`webgrab.exe` on Windows)

Flags work with two dashes or one (`--threads 4` = `-threads 4`), hyphens or underscores (`--crawl-depth` = `--crawl_depth`).
Short flags (`-t 4`, `-d out`, `-R`) take their value after a space and can't be combined.

```
webgrab <URL|FILE> [flags]
webgrab -r urls.txt [flags]
webgrab -w [flags]
```

## Flags
| short | long | |
|---|---|---|
| `-l` | `--list` | print links, don't download |
| `-T` | `--test` | print counts only |
| `-r FILE` | `--read FILE` | one URL per line |
| `-w` | `--watch` | download URLs copied to the clipboard |
| `-t N` | `--threads N` | parallel downloads (default 1) |
| `-d DIR` | `--dir DIR` | download folder |
| `-R` | `--recursive` | `--skip` and `--hash-existing` also look in subfolders |
| `-D N` | `--crawl-depth N` | crawl depth (default 0 = only the given page, see below) |
| | `--crawl-sleep MS` | minimum pause before every page request; the learned 429 pause is used instead when larger |
| | `--crawl-no-retry-429` | don't retry or learn on HTTP 429 (both are on by default) |
| | `--crawl-retries N` | max retries per page after HTTP 429 (default 6) |
| | `--external` | with `--crawl-depth`: also follow links to other hosts |
| | `--crawl-any a,b` | only follow pages whose URL contains ANY of these |
| | `--crawl-must a,b` | only follow pages whose URL contains ALL of these |
| | `--crawl-ignore a,b` | never follow pages whose URL contains any of these |
| `-u` | `--unique` | queue every URL once |
| `-n` | `--nodupes` | hash each download; same content already saved -> delete the new file |
| `-H` | `--hash-existing` | hash files already in `--dir` first |
| | `--db FILE.db` | SQLite file that remembers pages, files and how they connect; also the resume state (below) |
| | `--tree FILE.db` | print a tree of what a crawl downloaded, from its db (see below) |
| | `--graph FILE.html` | write an interactive graph of the `--db` (alone, or after a crawl) |
| `-s` | `--skip` | skip if a file with the same **name** exists |
| `-p` | `--replace` | overwrite same-name files (default: `name [1].ext`) |
| `-b URL` | `--base URL` | resolve relative links in a local .html |
| | `--timeout SECS` | |
| `-a` | `--any a,b` | file link must contain ANY of these |
| `-m` | `--must a,b` | file link must contain ALL of these |
| `-i` | `--ignore a,b` | file link must contain NONE of these |
| `-M` | `--media` | + `--video --image --other --codec` |
| `-q` `-v` `-f` | `--quiet --verbose --filename` | + `--compact`; `--colorless` turns colors off |

Defaults: colors are on (off when output goes to a file/pipe, with `--colorless`, or when `COLORLESS` is set);
downloads are counted as `DLED[done/total]` (and `DING[started/total]` with `--verbose`); every link must be
http(s). `--list` numbers links (`[1] url`) only on a terminal; piped output is one bare URL per line.

User-Agent: `--agent` (`webgrab`), `--agent-firefox`, `--agent-chromium`, `--agent-chrome`, `--agent-brave`
(the Chromium-family ones also send matching `Sec-CH-UA*` client hints), `--user-agent STRING` (wins).
Browser versions are constants at the top of `src/ua.rs`.

## Crawling (`--crawl-depth`)
Links on a page are split into **pages** and **files**:
- pages (crawled): `<a href>`, `<area href>`, `<iframe>`/`<frame>` links that look like web pages
  (no extension, `.html`, `.php`, ... , not media/archive/asset files)
- files (downloaded): every other link that passes your filters (`--media`, `--any`, `--must`, `--ignore` ...).
  Links inside `<img>`, `<video>`, `<audio>`, `<source>` (`src`, `srcset`, `poster`, `data-src` ...) count
  as media even without a file extension: `--image` takes `<img>` links, `--video` takes `<video>`/`<audio>`
  links, `--media` both. This works with `--crawl-depth 0` too. Files saved from such URLs get their
  extension from the response's Content-Type (`.jpg`, `.mp4`, ...).

`--crawl-depth 0` scans only the page you gave (as before), `--crawl-depth 1` also scans the pages it links to,
`--crawl-depth 2` the pages those link to, and so on. Each page is visited once, only on the start page's host
(`www.` ignored) unless `--external`; files are downloaded from any host. Files are downloaded as soon as a
page is scanned. Which pages are followed is decided by `--crawl-any/--crawl-must/--crawl-ignore` (`--ignore`
only concerns files); pages themselves are only saved with `--codec`. Use `--unique` so a file shown on many pages is fetched once. `--list --crawl-depth N` prints the
files found while crawling (progress goes to stderr); for a local .html file add `--base`.
**Rate limits:** HTTP 429 is handled by default. The page is retried after 1 s, 2 s, 4 s ... (max 60 s; a longer
`Retry-After` from the server wins, and the wait is never below the learned pause), each wait plus a random
offset (up to 20% + 250 ms). After `--crawl-retries` failed tries the page is counted as failed and the crawl
moves on. The crawler also **learns**: each 429 measures how long the failed request took and which pause was used
before it, and raises a calibrated pause to the largest of 1.5x the old one, 2x the failed pause, 2x the request time
and 500 ms (max 60 s). Every later page request waits `max(--crawl-sleep, calibrated pause)` + up to 10% random, so
after one 429 the crawl slows down instead of running into the limit again (raised again if it still happens).
`--crawl-sleep MS` is therefore the *minimum* pause; the calibrated pause is used when larger and is not saved
between runs. `--crawl-no-retry-429` switches retrying and learning off. These apply to page requests only; file
downloads are not throttled.
Resuming a crawl: with `--db` every crawled page is recorded once its files are done. On a re-run,
pages at the last depth that are already in the db are skipped; recorded pages *above* it are read again
only to find the pages below them (their files are skipped), otherwise an interrupted crawl could never
continue. Pages that failed (e.g. 429) are recorded as failed and retried. Raising `--crawl-depth`
between runs works the same way.

## Skip / resume
- `--unique` same URL once. `--nodupes` same content once. `--skip` same file *name* (only the name!
  Two different links with the same file name confuse `--skip` and `--replace`).
- `--db crawl.db` is a single SQLite file (WAL while running, one file after a clean finish). It is loaded at
  start and updated as things happen, so an interrupted run loses nothing. Re-running
  `webgrab --db crawl.db --nodupes --unique <url>` skips every URL already downloaded (or deleted as a
  duplicate) and uses the stored hashes for `--nodupes`. Failed downloads are retried.
  `--hash-existing` adds files already in `--dir` (status `existing`, no url). Old `--live-csv` files are not imported.

## The database
| table | one row per | main columns |
|---|---|---|
| `pages` | page URL (also pages only seen as a link: `crawled_at` is NULL) | `url`, `title`, `http_status`, `content_type`, `size`, `html_sha256`, `is_dupe`/`dupe_of`, `depth`, `error`, `discovered_at`, `crawled_at` |
| `media` | file URL | `url`, `kind` (image/video/file), `status` (queued/downloaded/duplicate/failed/existing), `file_path`, `mime_type`, `size`, `sha256`, `dupe_of`, `http_status`, `error`, `downloaded_at` |
| `links` | page -> page | `from_page`, `to_page` |
| `page_media` | page -> file it shows | `page_id`, `media_id` (recorded even when `--unique` skipped the download) |
| `redirects` | requested page -> final page | `from_page`, `to_page` |

Views for easy browsing: `v_page_media` (which page has which file), `v_links`, `v_redirects`,
`v_media_dupes`, `v_edges` (all relationships in one list). Open the file with any SQLite tool
(DB Browser for SQLite, Datasette, `sqlite3`).

**Graph:** `webgrab --db crawl.db --graph crawl.html` (or add `--graph` to a crawl) writes one standalone
HTML file: pages and files as nodes, links / has-file / redirects as edges, with search, filters and a
details panel on click; a checkbox groups each page's files into one "N files" node. The page loads the vis-network library from a CDN, so it needs internet when opened.

**Tree:** `webgrab --tree crawl.db` prints the downloads as a tree (no download happens):
```
◆ https://example.com/posts/  (2 pages, 4 files)
├─ ▸ /post/1/
│  ├─ /post/1/img1.jpg
│  └─ /post/1/img2.jpg
└─ ▸ /post/2/
   ├─ /post/2/img1.jpg
   └─ /post/2/img2.jpg
```
Roots are the pages the crawl started from, branches the pages they link to, leaves the files. Each page and
file is listed once (under the first page that reached it); branches without files are left out; duplicates and
failed downloads are marked (`≡ duplicate`, `✗ failed`). Piped output has no colors.

Exit code: 0 ok, 1 bad input / fatal, 2 some downloads failed.
