use crate::cli::Cli;
use url::Url;

pub const TYPES_VIDEO: &[&str] = &[
    ".mp4", ".webm", ".avi", ".mov", ".mkv", ".flv", ".mpeg", ".mpg", ".wmv", ".mp3", ".ogg",
];
pub const TYPES_IMAGE: &[&str] = &[
    ".jpg", ".jpeg", ".jpe", ".jiff", ".jfif", ".png", ".gif", ".ico", ".svg", ".bmp", ".dib",
    ".tif", ".tiff",
];
pub const TYPES_OTHER: &[&str] = &[".zip", ".rar", ".exe", ".swf", ".dll", ".txt"];
pub const TYPES_CODEC: &[&str] = &[
    ".asp", ".aspx", ".axd", ".asx", ".asmx", ".ashx", ".css", ".cfm", ".yaws", ".swf", ".html",
    ".htm", ".xhtml", ".jhtml", ".jsp", ".jspx", ".wss", ".do", ".action", ".js", ".pl", ".php",
    ".php4", ".php3", ".phtml", ".py", ".rb", ".rhtml", ".shtml", ".xml", ".rss", ".svg", ".cgi",
    ".dll",
];

/// Extensions that are files, never web pages (used to tell crawlable links from downloads).
const NON_PAGE_EXTS: &[&str] = &[
    ".css", ".js", ".mjs", ".json", ".xml", ".rss", ".atom", ".svg", ".ico", ".map", ".woff",
    ".woff2", ".ttf", ".otf", ".eot", ".pdf", ".doc", ".docx", ".xls", ".xlsx", ".ppt", ".pptx",
    ".csv", ".gz", ".tgz", ".tar", ".bz2", ".xz", ".7z", ".iso", ".apk", ".dmg", ".msi", ".bin",
    ".torrent",
];

/// Could this link be a web page we can crawl? (http(s), and not a media/archive/asset file).
/// No extension (`/gallery/`) and unknown extensions count as pages; the crawler double-checks
/// the Content-Type before reading a response.
pub fn is_page_link(link: &str) -> bool {
    if !is_http(link) {
        return false;
    }
    match crate::util::url_ext(link) {
        None => true,
        Some(e) => {
            let e = e.as_str();
            !(TYPES_VIDEO.contains(&e)
                || TYPES_IMAGE.contains(&e)
                || TYPES_OTHER.contains(&e)
                || NON_PAGE_EXTS.contains(&e))
        }
    }
}

pub fn is_http(link: &str) -> bool {
    Url::parse(link)
        .map(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or(false)
}

/// What the HTML element around a link says it is (`<img>` -> Image, `<video>`/`<audio>` -> Video).
/// Lets --image/--video/--media accept links that have no file extension.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hint {
    Image,
    Video,
}

/// Kind of file a link is, for the database: from its element, else from its extension.
pub fn media_kind(link: &str, hint: Option<Hint>) -> &'static str {
    match hint {
        Some(Hint::Image) => "image",
        Some(Hint::Video) => "video",
        None => match crate::util::url_ext(link) {
            Some(e) if TYPES_IMAGE.contains(&e.as_str()) => "image",
            Some(e) if TYPES_VIDEO.contains(&e.as_str()) => "video",
            _ => "file",
        },
    }
}

/// Keyword rules, case-insensitive substring matching:
///   any    = at least one must be present (if there are any)
///   must   = all must be present
///   ignore = none may be present
pub struct Keywords {
    any: Vec<String>,
    must: Vec<String>,
    ignore: Vec<String>,
}

impl Keywords {
    pub fn new(any: &[String], must: &[String], ignore: &[String]) -> Self {
        let lc = |v: &[String]| v.iter().map(|s| s.to_lowercase()).collect::<Vec<_>>();
        Self { any: lc(any), must: lc(must), ignore: lc(ignore) }
    }

    fn any_ok(&self, l: &str) -> bool {
        self.any.is_empty() || self.any.iter().any(|k| l.contains(k.as_str()))
    }

    fn must_ok(&self, l: &str) -> bool {
        self.must.iter().all(|k| l.contains(k.as_str()))
    }

    fn ignore_ok(&self, l: &str) -> bool {
        !self.ignore.iter().any(|k| l.contains(k.as_str()))
    }

    pub fn passes(&self, link: &str) -> bool {
        let l = link.to_lowercase();
        self.any_ok(&l) && self.must_ok(&l) && self.ignore_ok(&l)
    }
}

/// Rules for FILES (--any/--must/--ignore + --image/--video/--media/--other/--codec).
/// A link must always be a valid http(s) URL.
pub struct Filters {
    kw: Keywords,
    hint_image: bool,
    hint_video: bool,
}

impl Filters {
    pub fn new(cli: &Cli) -> Self {
        Self {
            kw: Keywords::new(&cli.any, &cli.must, &cli.ignore),
            hint_image: cli.media || cli.image,
            hint_video: cli.media || cli.video,
        }
    }

    pub fn passes(&self, link: &str, hint: Option<Hint>) -> bool {
        if !is_http(link) {
            return false;
        }
        let l = link.to_lowercase();
        if !self.kw.any_ok(&l) {
            // no extension/keyword match: still accepted if the element says it's wanted media
            let by_hint = match hint {
                Some(Hint::Image) => self.hint_image,
                Some(Hint::Video) => self.hint_video,
                None => false,
            };
            if !by_hint {
                return false;
            }
        }
        self.kw.must_ok(&l) && self.kw.ignore_ok(&l)
    }
}
