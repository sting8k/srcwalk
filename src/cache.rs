use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;

/// Cached outline entry.
struct CacheEntry {
    outline: Arc<str>,
}

/// Max number of cached parse trees before the cache is cleared wholesale.
/// Trees can be 3-10x source size in memory; 2000 files ≈ a few hundred MB.
/// Wholesale clear is simpler than LRU and adequate for intra-session scope.
const PARSE_CACHE_LIMIT: usize = 2000;

/// Outline + parse cache keyed by (canonical path, mtime). If the file changes,
/// mtime changes and the old entry is never hit again.
pub struct OutlineCache {
    entries: DashMap<(PathBuf, SystemTime), CacheEntry>,
    /// Tree-sitter parse cache. Separate DashMap since Tree has no text-key lookup.
    /// Cleared wholesale when it exceeds PARSE_CACHE_LIMIT entries.
    trees: DashMap<(PathBuf, SystemTime), tree_sitter::Tree>,
    tree_count: AtomicUsize,
}

impl Default for OutlineCache {
    fn default() -> Self {
        Self {
            entries: DashMap::new(),
            trees: DashMap::new(),
            tree_count: AtomicUsize::new(0),
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

    /// Get cached parse tree or parse and cache it. Returns None if parsing fails
    /// or the language can't be set. Tree is cheap-clone (refcount internally).
    ///
    /// Callers must pass content that matches `mtime` — stale content + fresh mtime
    /// would poison the cache. In practice both come from the same fs::metadata read.
    pub fn get_or_parse(
        &self,
        path: &Path,
        mtime: SystemTime,
        content: &str,
        lang: &tree_sitter::Language,
    ) -> Option<tree_sitter::Tree> {
        let key = (path.to_path_buf(), mtime);

        // Fast path: already cached.
        if let Some(tree) = self.trees.get(&key) {
            return Some(tree.clone());
        }

        // Miss: parse.
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(lang).ok()?;
        let tree = parser.parse(content, None)?;

        // Cap check before insert. When exceeded, clear wholesale — simpler than LRU,
        // and the common case is either small repos (never exceeded) or MCP sessions
        // that revisit the same hot files (clear then refill naturally).
        if self.tree_count.load(Ordering::Relaxed) >= PARSE_CACHE_LIMIT {
            self.trees.clear();
            self.tree_count.store(0, Ordering::Relaxed);
        }

        match self.trees.entry(key) {
            Entry::Occupied(e) => Some(e.get().clone()),
            Entry::Vacant(e) => {
                self.tree_count.fetch_add(1, Ordering::Relaxed);
                Some(e.insert(tree).value().clone())
            }
        }
    }
}
