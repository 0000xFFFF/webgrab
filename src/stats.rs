use std::sync::atomic::{AtomicU64, AtomicUsize};

/// Thread-safe counters.
#[derive(Default)]
pub struct Stats {
    pub total: AtomicUsize,      // every candidate found
    pub accepted: AtomicUsize,   // passed the filters
    pub queued: AtomicUsize,     // downloads handed to the workers so far ("to download")
    pub processed: AtomicUsize,  // download attempts started
    pub dled: AtomicUsize,       // files saved
    pub bytes: AtomicU64,        // bytes saved
    pub renamed: AtomicUsize,    // name clash -> "name [1].ext"
    pub replaced: AtomicUsize,   // name clash -> overwritten (--replace)
    pub hash_dupes: AtomicUsize, // identical content -> deleted (--nodupes)
    pub skipped: AtomicUsize,    // skipped by the --skip* rules
    pub disposed: AtomicUsize,   // rejected by filters / --unique
    pub failed: AtomicUsize,
    pub pages: AtomicUsize,      // pages crawled
    pub pages_todo: AtomicUsize, // pages found that have to be crawled (start page + followed links so far)
    pub pages_done: AtomicUsize, // of those: finished (crawled, skipped, failed or not html)
    pub page_dupes: AtomicUsize, // pages with the same content as an earlier page
}
