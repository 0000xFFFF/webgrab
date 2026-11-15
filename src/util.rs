use percent_encoding::percent_decode_str;
use url::Url;

/// 1536 -> "1.5 KB"
pub fn human(len: f64) -> String {
    const SIZES: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut len = len;
    let mut order = 0;
    while len >= 1024.0 && order < SIZES.len() - 1 {
        order += 1;
        len /= 1024.0;
    }
    let s = format!("{:.2}", len);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{} {}", if s.is_empty() { "0" } else { s }, SIZES[order])
}

/// Make a string safe to use as a file name on Windows/Linux/macOS.
pub fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    s.trim_end_matches(|c| c == ' ' || c == '.')
        .trim_start()
        .to_string()
}

/// File name from the path part of a URL (percent-decoded, sanitized).
/// Works for relative / non-URL strings too (falls back to "after the last /").
pub fn file_name_from_url(link: &str) -> Option<String> {
    let raw: String = match Url::parse(link) {
        Ok(u) => u
            .path_segments()
            .and_then(|s| s.last())
            .unwrap_or("")
            .to_string(),
        Err(_) => link
            .split(|c| c == '?' || c == '#')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string(),
    };
    let dec = percent_decode_str(&raw).decode_utf8_lossy();
    let s = sanitize(&dec);
    if s.is_empty() { None } else { Some(s) }
}

/// Lower-cased ".ext" of the URL's path, if any.
pub fn url_ext(link: &str) -> Option<String> {
    let name = file_name_from_url(link)?;
    let i = name.rfind('.')?;
    Some(name[i..].to_lowercase())
}

/// "a.tar.gz" -> ("a.tar", ".gz"), "noext" -> ("noext", "")
pub fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// All regular files in `dir` (and below, when `recursive`). Symlinked dirs are not followed.
pub fn walk_files(dir: &std::path::Path, recursive: bool) -> Vec<std::path::PathBuf> {
    fn go(dir: &std::path::Path, recursive: bool, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                match e.file_type() {
                    Ok(ft) if ft.is_dir() => {
                        if recursive {
                            go(&p, true, out);
                        }
                    }
                    Ok(ft) if ft.is_file() => out.push(p),
                    _ => {}
                }
            }
        }
    }
    let mut out = Vec::new();
    go(dir, recursive, &mut out);
    out
}

/// "http://x/a#frag" -> "http://x/a"
pub fn strip_fragment(s: &str) -> &str {
    s.split('#').next().unwrap_or(s)
}
