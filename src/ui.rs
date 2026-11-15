use std::io::{IsTerminal, Write};
use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum K {
    Def, // default
    Inf, // information
    Top, // headers
    Cur, // current / "DING"
    Pri, // downloaded
    War, // warning
    Eri, // error counts (stdout)
    Err, // error (stderr)
    Cer, // critical error (stderr, shown even in compact mode)
}

/// Thread-safe console printer (one line at a time, optional ANSI colors).
pub struct Printer {
    color_out: bool,
    color_err: bool,
    compact: bool,
    lock: Mutex<()>,
}

/// Is stdout a terminal (not a pipe / file)?
pub fn stdout_is_tty() -> bool {
    std::io::stdout().is_terminal()
}

impl Printer {
    /// Colors are on by default; --nocolor and the NO_COLOR variable turn them off, and so does
    /// redirecting the output to a file or a pipe (so logs don't fill up with escape codes).
    pub fn new(nocolor: bool, compact: bool) -> Self {
        let off = nocolor || std::env::var_os("NO_COLOR").map_or(false, |v| !v.is_empty());
        Self {
            color_out: !off && std::io::stdout().is_terminal(),
            color_err: !off && std::io::stderr().is_terminal(),
            compact,
            lock: Mutex::new(()),
        }
    }

    fn code(k: K) -> &'static str {
        match k {
            K::Def => "\x1b[37m",
            K::Inf => "\x1b[33m",
            K::Top => "\x1b[36m",
            K::Cur => "\x1b[32m",
            K::Pri => "\x1b[92m",
            K::War => "\x1b[93m",
            K::Eri | K::Err | K::Cer => "\x1b[91m",
        }
    }

    /// Normal message: hidden in --compact mode (except critical errors).
    pub fn say(&self, k: K, text: &str) {
        if self.compact && k != K::Cer {
            return;
        }
        self.always(k, text);
    }

    /// Message that ignores --compact.
    pub fn always(&self, k: K, text: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let color = if matches!(k, K::Err | K::Cer) { self.color_err } else { self.color_out };
        let line = if color {
            format!("{}{}\x1b[0m", Self::code(k), text)
        } else {
            text.to_string()
        };
        if matches!(k, K::Err | K::Cer) {
            let _ = writeln!(std::io::stderr(), "{}", line);
        } else {
            let _ = writeln!(std::io::stdout().lock(), "{}", line);
        }
    }

    /// Raw stdout line, no color, no compact handling (pipe friendly: --list output).
    pub fn plain(&self, text: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(std::io::stdout().lock(), "{}", text);
    }

    /// Rewrite the current line (compact counters).
    pub fn ticker(&self, text: &str) {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = std::io::stdout().lock();
        let _ = write!(out, "\r{}", text);
        let _ = out.flush();
    }
}
