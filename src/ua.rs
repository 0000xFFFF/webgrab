use crate::cli::Cli;

// Bump these now and then so the presets keep looking like current browsers.
const FIREFOX_VER: u32 = 144;
const CHROME_VER: u32 = 141; // Chrome/Chromium/Brave all share the Chromium major version

#[cfg(windows)]
const OS: (&str, &str, &str) = (
    "Windows NT 10.0; Win64; x64",
    "Windows NT 10.0; Win64; x64",
    "Windows",
);
#[cfg(target_os = "macos")]
const OS: (&str, &str, &str) = (
    "Macintosh; Intel Mac OS X 10_15_7",
    "Macintosh; Intel Mac OS X 10.15",
    "macOS",
);
#[cfg(not(any(windows, target_os = "macos")))]
const OS: (&str, &str, &str) = ("X11; Linux x86_64", "X11; Linux x86_64", "Linux");

fn chromium_ua() -> String {
    format!(
        "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{}.0.0.0 Safari/537.36",
        OS.0, CHROME_VER
    )
}

/// Chromium-family browsers also send client hints; sites that check them expect
/// the brand list to match the User-Agent.
fn chromium_headers(brands: &[&str]) -> Vec<(String, String)> {
    let v = CHROME_VER;
    let mut list: Vec<String> = brands
        .iter()
        .map(|b| format!("\"{}\";v=\"{}\"", b, v))
        .collect();
    list.push("\"Not_A Brand\";v=\"24\"".to_string());
    vec![
        ("User-Agent".into(), chromium_ua()),
        ("Sec-CH-UA".into(), list.join(", ")),
        ("Sec-CH-UA-Mobile".into(), "?0".into()),
        ("Sec-CH-UA-Platform".into(), format!("\"{}\"", OS.2)),
    ]
}

/// Headers added to every request. Priority: --user-agent > brave > chrome > chromium > firefox > --agent.
pub fn headers(cli: &Cli) -> Vec<(String, String)> {
    if let Some(ua) = &cli.user_agent {
        vec![("User-Agent".into(), ua.clone())]
    } else if cli.agent_brave {
        chromium_headers(&["Brave", "Chromium"])
    } else if cli.agent_chrome {
        chromium_headers(&["Google Chrome", "Chromium"])
    } else if cli.agent_chromium {
        chromium_headers(&["Chromium"])
    } else if cli.agent_firefox {
        vec![(
            "User-Agent".into(),
            format!(
                "Mozilla/5.0 ({}; rv:{v}.0) Gecko/20100101 Firefox/{v}.0",
                OS.1,
                v = FIREFOX_VER
            ),
        )]
    } else if cli.agent {
        vec![("User-Agent".into(), "webgrab".into())]
    } else {
        vec![]
    }
}
