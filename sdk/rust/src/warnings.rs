//! The warning channel: `tracing::warn!` unless the client was built with
//! another sink, and each distinct warning once for the client's life, so a
//! platform answering the same thing on every export is heard once.

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Mutex;

/// How many distinct warnings are remembered before the set starts over.
const WARNED_MAX: usize = 1024;

pub(crate) struct Warner {
    sink: Box<dyn Fn(&str) + Send + Sync>,
    warned: Mutex<HashSet<String>>,
}

impl Warner {
    pub(crate) fn new(sink: Box<dyn Fn(&str) + Send + Sync>) -> Self {
        Self {
            sink,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// `text`, the first time `key` is seen.
    pub(crate) fn once(&self, key: String, text: &str) {
        // The lock is released before the sink runs: a sink is the
        // application's, and may take its time.
        let fresh = {
            let mut warned = crate::lock(&self.warned);
            // Bounded: a server naming ever-new attributes would otherwise
            // grow it for the life of the process; past the cap, warnings may
            // repeat.
            if !warned.contains(&key) && warned.len() >= WARNED_MAX {
                warned.clear();
            }
            warned.insert(key)
        };
        if fresh {
            self.give(text);
        }
    }

    /// `text`, every time: for a count, which is never the same warning twice.
    pub(crate) fn warn(&self, text: &str) {
        self.give(text);
    }

    fn give(&self, text: &str) {
        // The sink is the application's: a panic in it must not reach the agent.
        let _ = panic::catch_unwind(AssertUnwindSafe(|| (self.sink)(text)));
    }
}
