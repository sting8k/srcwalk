use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;

/// Cached outline entry.
struct CacheEntry {
    outline: Arc<str>,
}

/// Outline cache keyed by (canonical path, mtime). If the file changes,
/// mtime changes and the old entry is never hit again.
///
/// Value is `Arc<str>` — inline string data in the Arc allocation,
/// one less indirection than `Arc<String>`.
pub struct OutlineCache {
    entries: DashMap<(PathBuf, SystemTime), CacheEntry>,
}

impl Default for OutlineCache {
    fn default() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }
}

impl OutlineCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get cached outline or compute and cache it. Accepts `&Path` (not `&PathBuf`).
    /// Uses `entry()` API to avoid TOCTOU race between get and insert.
    pub fn get_or_compute(
        &self,
        path: &Path,
        mtime: SystemTime,
        compute: impl FnOnce() -> String,
    ) -> Arc<str> {
        match self.entries.entry((path.to_path_buf(), mtime)) {
            Entry::Occupied(e) => Arc::clone(&e.get().outline),
            Entry::Vacant(e) => {
                let outline: Arc<str> = compute().into();
                e.insert(CacheEntry {
                    outline: Arc::clone(&outline),
                });
                outline
            }
        }
    }
}
