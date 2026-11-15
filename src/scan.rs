use crate::filter::Hint;
use regex::Regex;
use std::sync::OnceLock;
use url::Url;

fn re_dq() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#""([^"]*)""#).unwrap())
}
fn re_sq() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"'([^']*)'"#).unwrap())
}
fn re_css() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)url\(\s*['"]?([^'")\s]+)['"]?\s*\)"#).unwrap())
}
fn re_href() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?i)<(?:a|area)\b[^>]*?\bhref\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
            .unwrap()
    })
}
fn re_frame() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r#"(?i)<(?:iframe|frame)\b[^>]*?\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#,
        )
        .unwrap()
    })
}
fn re_tag() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r#"(?is)</(video|audio|picture)\s*>|<(img|video|audio|source|picture)\b([^>]*)>"#,
        )
        .unwrap()
    })
}
fn re_attr() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#)
            .unwrap()
    })
}
fn re_comma_ws() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r",\s+").unwrap())
}

/// <title> of the page (whitespace collapsed, entities decoded).
pub fn title(html: &str) -> String {
    // ASCII lowercasing keeps byte offsets identical to the original string.
    let lower = html.to_ascii_lowercase();
    let Some(start) = lower.find("<title") else {
        return String::new();
    };
    let Some(gt) = lower[start..].find('>') else {
        return String::new();
    };
    let open_end = start + gt + 1;
    let Some(end) = lower[open_end..].find("</title>") else {
        return String::new();
    };
    let raw = html_escape::decode_html_entities(&html[open_end..open_end + end]).to_string();
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A link found in the page. `hint` = what its HTML element says it is (if it's media).
#[derive(Debug, Clone)]
pub struct Found {
    pub url: String,
    pub hint: Option<Hint>,
}

/// Attribute values of <img>/<video>/<audio>/<source> that point to media:
/// (position of the value in the page, raw value, kind).
fn element_hits(html: &str) -> Vec<(usize, String, Hint)> {
    let mut out = Vec::new();
    let mut parent: Option<&'static str> = None; // enclosing <video>/<audio>/<picture>
    for c in re_tag().captures_iter(html) {
        if c.get(1).is_some() {
            parent = None; // closing tag
            continue;
        }
        let tag = c.get(2).unwrap().as_str().to_ascii_lowercase();
        let attrs_m = c.get(3).unwrap();
        let mut attrs: Vec<(String, usize, String)> = Vec::new();
        for a in re_attr().captures_iter(attrs_m.as_str()) {
            let v = a.get(2).or_else(|| a.get(3)).or_else(|| a.get(4)).unwrap();
            attrs.push((
                a[1].to_ascii_lowercase(),
                attrs_m.start() + v.start(),
                v.as_str().to_string(),
            ));
        }
        let mime = attrs
            .iter()
            .find(|(n, _, _)| n == "type")
            .map(|(_, _, v)| v.to_lowercase());

        let mut push = |names: &[&str], kind: Hint| {
            for (n, pos, v) in &attrs {
                if names.contains(&n.as_str()) {
                    out.push((*pos, v.clone(), kind));
                }
            }
        };
        const LAZY: [&str; 6] = [
            "src",
            "srcset",
            "data-src",
            "data-srcset",
            "data-original",
            "data-lazy-src",
        ];
        match tag.as_str() {
            "img" => push(&LAZY, Hint::Image),
            "video" => {
                parent = Some("video");
                push(&["src", "data-src"], Hint::Video);
                push(&["poster"], Hint::Image);
            }
            "audio" => {
                parent = Some("audio");
                push(&["src", "data-src"], Hint::Video);
            }
            "picture" => parent = Some("picture"),
            "source" => {
                let kind = match mime.as_deref() {
                    Some(m) if m.starts_with("image/") => Some(Hint::Image),
                    Some(m) if m.starts_with("video/") || m.starts_with("audio/") => {
                        Some(Hint::Video)
                    }
                    _ => match parent {
                        Some("video") | Some("audio") => Some(Hint::Video),
                        Some("picture") => Some(Hint::Image),
                        _ => None,
                    },
                };
                if let Some(k) = kind {
                    push(&LAZY, k);
                }
            }
            _ => {}
        }
    }
    out
}

/// Every "quoted" / 'quoted' string and css url(...) in the page, in document order,
/// resolved against `base` when it is relative. Also understands srcset lists.
/// Links that sit in a media element (<img src>, <video src>, <source srcset> ...) carry a `hint`.
pub fn extract(base: Option<&Url>, html: &str) -> Vec<Found> {
    let mut found: Vec<(usize, String, Option<Hint>)> = Vec::new();
    for re in [re_dq(), re_sq(), re_css()] {
        for c in re.captures_iter(html) {
            let m = c.get(1).unwrap();
            found.push((m.start(), m.as_str().to_string(), None));
        }
    }
    for (pos, raw, kind) in element_hits(html) {
        found.push((pos, raw, Some(kind)));
    }
    // the quoted-string scan and the element scan see the same value at the same position:
    // keep one, the one with the hint
    found.sort_by_key(|(pos, _, hint)| (*pos, hint.is_none()));
    found.dedup_by_key(|(pos, _, _)| *pos);

    let mut out = Vec::new();
    for (_, raw, hint) in found {
        for cand in candidates(&raw) {
            if let Some(u) = resolve(base, &cand) {
                out.push(Found { url: u, hint });
            }
        }
    }
    out
}

fn plausible(s: &str) -> bool {
    !s.starts_with('#')
        && !s
            .chars()
            .any(|c| "<>{}\"'\\".contains(c) || c.is_whitespace())
        && (s.contains('/') || s.contains('.'))
}

fn is_descriptor(t: &str) -> bool {
    match t.chars().last() {
        Some('w') | Some('x') | Some('h') => t[..t.len() - 1].parse::<f32>().is_ok(),
        _ => false,
    }
}

/// One quoted string -> zero or more URL-ish candidates.
fn candidates(raw: &str) -> Vec<String> {
    if raw.len() > 2048 {
        return vec![];
    }
    let mut s = html_escape::decode_html_entities(raw).to_string();
    if s.contains('\\') {
        s = s
            .replace("\\/", "/")
            .replace("\\u0026", "&")
            .replace("\\u002F", "/")
            .replace("\\u002f", "/");
    }
    let s = s.trim();
    if s.is_empty() {
        return vec![];
    }
    if !s.chars().any(char::is_whitespace) {
        return if plausible(s) {
            vec![s.to_string()]
        } else {
            vec![]
        };
    }

    // srcset="a.jpg 480w, b.jpg 800w"
    let mut urls: Vec<&str> = Vec::new();
    let mut has_desc = false;
    for part in re_comma_ws().split(s) {
        let toks: Vec<&str> = part.split_whitespace().collect();
        match toks.len() {
            1 => urls.push(toks[0]),
            2 if is_descriptor(toks[1]) => {
                has_desc = true;
                urls.push(toks[0]);
            }
            _ => return vec![],
        }
    }
    if has_desc {
        urls.into_iter()
            .filter(|u| plausible(u))
            .map(String::from)
            .collect()
    } else {
        vec![]
    }
}

fn resolve(base: Option<&Url>, s: &str) -> Option<String> {
    if let Ok(u) = Url::parse(s) {
        // "scheme://..." only; things like "background:url(x)" / "mailto:" / "data:" are not links
        return if s.contains("://") {
            Some(u.to_string())
        } else {
            None
        };
    }
    match base {
        Some(b) => b.join(s).ok().map(|u| u.to_string()),
        None => Some(s.to_string()), // local file without --base: keep as written
    }
}

/// Links the crawler may follow: `<a href>`, `<area href>`, `<iframe src>`, `<frame src>`,
/// resolved against `base`, fragments removed, only things that look like web pages.
/// (Everything else found by `extract` is a download candidate.)
pub fn extract_pages(base: Option<&Url>, html: &str) -> Vec<String> {
    let mut found: Vec<(usize, String)> = Vec::new();
    for re in [re_href(), re_frame()] {
        for c in re.captures_iter(html) {
            let m = c.get(1).or_else(|| c.get(2)).or_else(|| c.get(3));
            if let Some(m) = m {
                found.push((m.start(), m.as_str().to_string()));
            }
        }
    }
    found.sort_by_key(|(pos, _)| *pos);

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (_, raw) in found {
        let dec = html_escape::decode_html_entities(&raw).to_string();
        let s = dec.trim();
        if s.is_empty() || s.starts_with('#') || s.len() > 2048 {
            continue;
        }
        let Some(u) = resolve(base, s) else { continue };
        let u = crate::util::strip_fragment(&u).to_string();
        if crate::filter::is_page_link(&u) && seen.insert(u.clone()) {
            out.push(u);
        }
    }
    out
}
