use crate::cli::Cli;
use crate::util::walk_files;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

/// --skip: exact file names that already exist (in --dir, and subdirectories with -R).
/// Only NAMES are compared, never content.
pub struct Skipper {
    active: bool,
    names: Mutex<HashSet<String>>,
}

impl Skipper {
    pub fn new(cli: &Cli, dir: &Path) -> Self {
        let s = Skipper {
            active: cli.skip,
            names: Mutex::new(HashSet::new()),
        };
        if cli.skip {
            for p in walk_files(dir, cli.recursive) {
                if let Some(n) = p.file_name() {
                    s.add(&n.to_string_lossy());
                }
            }
        }
        s
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn add(&self, name: &str) {
        self.names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string());
    }

    pub fn contains(&self, name: &str) -> bool {
        self.names
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(name)
    }
}
