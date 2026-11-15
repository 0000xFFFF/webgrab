use crate::filter::{TYPES_CODEC, TYPES_IMAGE, TYPES_OTHER, TYPES_VIDEO};
use clap::Parser;
use std::ffi::OsString;
use std::path::PathBuf;

const EXAMPLES: &str = "\
Flags can be written with two dashes (--threads 4) or one dash (-threads 4).
Hyphens and underscores are interchangeable (--skip-all = --skip_all).
Short flags (-t, -d, -R ...) take their value after a space and can't be combined.

SKIP / DUPLICATE RULES (what each one checks):
  --unique     the same URL is queued only once
  --nodupes    the same file CONTENT (sha256) is kept only once
  --skip       the same file NAME already exists in --dir
               (only the name is compared! if two different links give different files
               with the same name, --skip and --replace can treat them as one file)
  --db         remembers every page, file and hash it handled, so re-running the same command resumes

EXAMPLES:
  webgrab https://example.com/gallery --media --threads 8
  webgrab https://example.com/gallery --list --image --unique
  webgrab https://example.com/gallery --crawl-depth 2 --image --unique -t 8 -d pics
  webgrab https://example.com/gallery -D 3 --media -u --crawl-sleep 1500
  webgrab https://example.com/gallery --crawl-depth 1 --list --media --unique
  webgrab page.html --list --base https://example.com/ --video
  webgrab --db crawl.db --nodupes --unique download_links.html
  webgrab https://example.com -D 2 --media -u -n -t 4 -d pics --db crawl.db --graph crawl.html
  webgrab --db crawl.db --graph crawl.html          (just draw the graph of an earlier crawl)
  webgrab --read urls.txt --media -t 4 -d downloads --db log.db --hash-existing -R
  webgrab \"https://www.youtube.com/watch?v=XXXXXXXXXXX\" --image --unique --must maxres";

#[derive(Parser, Debug)]
#[command(
    name = "webgrab",
    version,
    about = "Web page file scanner / downloader",
    arg_required_else_help = true,
    after_help = EXAMPLES
)]
pub struct Cli {
    /// Page URL, local .html file, or a direct link to a file
    #[arg(value_name = "URL|FILE")]
    pub input: Option<String>,

    // ---------------------------------------------------------------- modes
    /// Only print the links found (no downloading). Great for reading an .html file
    #[arg(short = 'l', long, help_heading = "Modes")]
    pub list: bool,

    /// No downloading, just print counts (size, title, total/accepted/rejected)
    #[arg(short = 'T', long, help_heading = "Modes")]
    pub test: bool,

    /// Read a text file and treat every line as a URL (empty lines and '#' lines ignored)
    #[arg(short = 'r', long, value_name = "FILE", help_heading = "Modes")]
    pub read: Option<PathBuf>,

    /// Download URLs as they are copied to the clipboard
    #[arg(short = 'w', long, help_heading = "Modes")]
    pub watch: bool,

    // ------------------------------------------------------------- download
    /// How many files to download at the same time
    #[arg(
        short = 't',
        long,
        default_value_t = 1,
        value_name = "NUM",
        help_heading = "Download"
    )]
    pub threads: usize,

    /// Save files into this directory (created if missing)
    #[arg(short = 'd', long, value_name = "DIR", help_heading = "Download")]
    pub dir: Option<PathBuf>,

    /// Search subdirectories too (for --skip and --hash-existing)
    #[arg(short = 'R', long, help_heading = "Download")]
    pub recursive: bool,

    /// Only queue every URL once
    #[arg(short = 'u', long, help_heading = "Download")]
    pub unique: bool,

    /// Keep file content unique: hash every download, delete it if the same content was already saved
    #[arg(short = 'n', long, help_heading = "Download")]
    pub nodupes: bool,

    /// Hash the files already in --dir (and in subdirectories with -R) before downloading
    ///
    /// The hashes feed --nodupes; with --db the files are also added to the database
    /// (status 'existing', no url).
    #[arg(short = 'H', long, help_heading = "Download")]
    pub hash_existing: bool,

    /// SQLite database that remembers the crawl: pages, files and how they are connected
    ///
    /// One single file (created if missing, only ever added to), updated live after every page
    /// and file. Tables: pages (url, title, http status, html hash ...), media (url, saved path,
    /// mime type, size, sha256, status ...), links (page -> page), page_media (page -> file),
    /// redirects (page -> page); views v_page_media, v_links, v_edges ... make it easy to read.
    /// URLs it already holds are skipped and its hashes feed --nodupes, so re-running an
    /// interrupted command continues where it stopped (failed ones are retried).
    /// Open it with any SQLite tool (DB Browser for SQLite, Datasette, the sqlite3 CLI) or draw it
    /// with --graph.
    #[arg(long, value_name = "FILE.db", help_heading = "Download")]
    pub db: Option<PathBuf>,

    /// Write an interactive graph (one standalone .html file) of the --db: which page links to which
    /// page, which files each page has, redirects, duplicates
    ///
    /// Runs after the download, or on its own: webgrab --db crawl.db --graph crawl.html
    #[arg(long, value_name = "FILE.html", help_heading = "Download")]
    pub graph: Option<PathBuf>,

    /// If the file exists, replace it (default: save as "name [1].ext")
    ///
    /// Only the file NAME is compared: if two different links give different files with
    /// the same name, --replace will overwrite one with the other.
    #[arg(short = 'p', long, help_heading = "Download")]
    pub replace: bool,

    /// Skip a URL if a file with exactly the same NAME already exists in --dir
    ///
    /// Only the file name is checked, not the content. If two different links point to
    /// different files that happen to share a name, --skip (and --replace) can be
    /// problematic: the second one is skipped/overwritten. Use --nodupes with --db
    /// to track real content. Add -R to also look in subdirectories.
    #[arg(short = 's', long, help_heading = "Download")]
    pub skip: bool,

    /// Base URL used to resolve relative links when the input is a local .html file
    #[arg(short = 'b', long, value_name = "URL", help_heading = "Download")]
    pub base: Option<String>,

    /// Seconds without data before a request is abandoned
    #[arg(
        long,
        default_value_t = 60,
        value_name = "SECS",
        help_heading = "Download"
    )]
    pub timeout: u64,

    // ---------------------------------------------------------------- crawl
    /// How deep to crawl: 0 = only the given page, 1 = also the pages it links to, 2 = and theirs ...
    ///
    /// Links are split into pages (crawled: <a href>, frames) and files (downloaded: everything
    /// else that passes your filters). Pages are followed only on the same host unless
    /// --external is given, each page once, and only if they pass the --crawl-any/--crawl-must/
    /// --crawl-ignore filters. Use --unique so a file shown on many pages is downloaded once.
    /// Pages are not saved unless you add --codec. Applies to a page URL / .html file, not --read.
    #[arg(
        short = 'D',
        long,
        default_value_t = 0,
        value_name = "NUM",
        help_heading = "Crawl"
    )]
    pub crawl_depth: usize,

    /// Only follow pages whose URL contains ANY of these (comma separated, repeatable)
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Crawl"
    )]
    pub crawl_any: Vec<String>,

    /// Only follow pages whose URL contains ALL of these
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Crawl"
    )]
    pub crawl_must: Vec<String>,

    /// Never follow pages whose URL contains any of these (e.g. --crawl-ignore logout,login)
    #[arg(
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Crawl"
    )]
    pub crawl_ignore: Vec<String>,

    /// Wait this many milliseconds before every page request (avoids HTTP 429)
    #[arg(long, default_value_t = 0, value_name = "MS", help_heading = "Crawl")]
    pub crawl_sleep: u64,

    /// Disable retries when a page responds with HTTP 429.
    ///
    /// By default, the crawler retries with exponential backoff (1s, 2s, 4s ... max 60s).
    /// The server's `Retry-After` header is respected if it requests a longer wait,
    /// and a random offset is added to each wait to avoid synchronized retries.
    #[arg(long, help_heading = "Crawl")]
    pub crawl_no_retry_429: bool,

    /// With how many times to retry one page before giving up
    #[arg(long, default_value_t = 6, value_name = "NUM", help_heading = "Crawl")]
    pub crawl_retries: u32,

    /// Also follow links to other hosts (default: stay on the start page's host)
    #[arg(long, help_heading = "Crawl")]
    pub external: bool,

    // ---------------------------------------------------------- user agent
    /// Send "User-Agent: webgrab"
    #[arg(long, help_heading = "User-Agent")]
    pub agent: bool,
    /// Pretend to be Firefox
    #[arg(long, help_heading = "User-Agent")]
    pub agent_firefox: bool,
    /// Pretend to be Chromium (Linux)
    #[arg(long, help_heading = "User-Agent")]
    pub agent_chromium: bool,
    /// Pretend to be Google Chrome
    #[arg(long, help_heading = "User-Agent")]
    pub agent_chrome: bool,
    /// Pretend to be Brave (Chrome UA + Brave client hints)
    #[arg(long, help_heading = "User-Agent")]
    pub agent_brave: bool,
    /// Send this exact User-Agent (wins over the presets)
    #[arg(long, value_name = "STRING", help_heading = "User-Agent")]
    pub user_agent: Option<String>,

    // -------------------------------------------------------------- filters
    /// Keep only files whose URL contains ANY of these (comma separated, repeatable)
    #[arg(
        short = 'a',
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Filters"
    )]
    pub any: Vec<String>,

    /// Keep only files whose URL contains ALL of these
    #[arg(
        short = 'm',
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Filters"
    )]
    pub must: Vec<String>,

    /// Drop files whose URL contains any of these
    #[arg(
        short = 'i',
        long,
        value_delimiter = ',',
        value_name = "KEYWORD",
        help_heading = "Filters"
    )]
    pub ignore: Vec<String>,

    /// --video --image --other
    #[arg(short = 'M', long, help_heading = "Filters")]
    pub media: bool,
    /// Video/audio extensions (.mp4 .webm .mkv .mp3 ...)
    #[arg(long, help_heading = "Filters")]
    pub video: bool,
    /// Image extensions (.jpg .png .gif .svg ...)
    #[arg(long, help_heading = "Filters")]
    pub image: bool,
    /// Other extensions (.zip .rar .exe .txt ...)
    #[arg(long, help_heading = "Filters")]
    pub other: bool,
    /// Page/code extensions (.html .css .js .php ...)
    #[arg(long, help_heading = "Filters")]
    pub codec: bool,

    // -------------------------------------------------------------- display
    /// Only print numbers (on a single line)
    #[arg(long, help_heading = "Display")]
    pub compact: bool,
    /// Don't print errors/warnings (only successful downloads)
    #[arg(short = 'q', long, help_heading = "Display")]
    pub quiet: bool,
    /// More detail (DING lines, table output with --list, debug block at the end)
    #[arg(short = 'v', long, help_heading = "Display")]
    pub verbose: bool,
    /// With --list: print only the file names
    #[arg(short = 'f', long, help_heading = "Display")]
    pub filename: bool,
    /// No colors (they are on by default, except when the output goes to a file or pipe; COLORLESS works too)
    #[arg(long, help_heading = "Display")]
    pub colorless: bool,
}

impl Cli {
    /// Expand the presets (--media, ...) into the plain options.
    pub fn resolve(&mut self) {
        let mut add = |list: &[&str]| self.any.extend(list.iter().map(|s| s.to_string()));
        let (media, video, image, other, codec) =
            (self.media, self.video, self.image, self.other, self.codec);
        if media || video {
            add(TYPES_VIDEO);
        }
        if media || image {
            add(TYPES_IMAGE);
        }
        if media || other {
            add(TYPES_OTHER);
        }
        if codec {
            add(TYPES_CODEC);
        }
        self.any.retain(|s| !s.is_empty());
        self.must.retain(|s| !s.is_empty());
        self.ignore.retain(|s| !s.is_empty());
        self.crawl_any.retain(|s| !s.is_empty());
        self.crawl_must.retain(|s| !s.is_empty());
        self.crawl_ignore.retain(|s| !s.is_empty());
        self.threads = self.threads.clamp(1, 256);
    }
}

/// Lets `-flag`, `--flag`, `--flag_name` and `/?` all work.
/// Single dash + more than one letter is treated as a long flag; real short flags (-t, -R, -h, -V) are untouched.
pub fn normalized_args() -> Vec<OsString> {
    std::env::args_os()
        .enumerate()
        .map(|(i, a)| {
            if i == 0 {
                return a;
            }
            let Some(s) = a.to_str() else { return a };
            if s == "/?" {
                return OsString::from("--help");
            }
            let body = if let Some(r) = s.strip_prefix("--") {
                r
            } else if let Some(r) = s.strip_prefix('-') {
                if r.chars().count() > 1
                    && r.chars().next().map_or(false, |c| c.is_ascii_alphabetic())
                {
                    r
                } else {
                    return a;
                }
            } else {
                return a;
            };
            if body.is_empty() {
                return a; // a bare "--"
            }
            let (name, rest) = match body.split_once('=') {
                Some((n, v)) => (n, format!("={}", v)),
                None => (body, String::new()),
            };
            OsString::from(format!("--{}{}", name.replace('_', "-"), rest))
        })
        .collect()
}
