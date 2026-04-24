use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::cache::OutlineCache;
use crate::lang::detect_file_type;
use crate::read::outline;
use crate::types::{estimate_tokens, FileType};

struct WalkConfig {
    hidden: bool,
    git_ignore: bool,
    git_global: bool,
    git_exclude: bool,
    ignore: bool,
    parents: bool,
}

/// Build the "# Note:" header line listing which ignore sources the walker
/// honours, derived from the actual `WalkConfig` (no hardcoded copy).
fn format_walk_note(cfg: &WalkConfig) -> String {
    let mut respects: Vec<&'static str> = Vec::new();
    if cfg.git_ignore {
        respects.push(".gitignore");
    }
    if cfg.git_exclude {
        respects.push(".git/info/exclude");
    }
    if cfg.git_global {
        respects.push("core.excludesFile");
    }
    if cfg.ignore {
        respects.push(".ignore");
    }
    let scope_word = if cfg.parents {
        "+ parents"
    } else {
        "scope only"
    };

    let respects_part = if respects.is_empty() {
        "no ignore files".to_string()
    } else {
        format!("{} ({scope_word})", respects.join(", "))
    };

    let hidden_part = if cfg.hidden {
        "dotfiles excluded"
    } else {
        "dotfiles included"
    };

    format!(
        "# Note: respects {respects_part}; {hidden_part}; built-in SKIP_DIRS still apply \
         (target, node_modules, …). Use `srcwalk <path>` to inspect an ignored file directly.\n",
    )
}

/// Generate a structural codebase map.
/// By default files show compact token estimates; symbol names are opt-in.
#[must_use]
pub fn generate(
    scope: &Path,
    depth: usize,
    budget: Option<u64>,
    cache: &OutlineCache,
    include_symbols: bool,
) -> String {
    let mut tree: BTreeMap<PathBuf, Vec<FileEntry>> = BTreeMap::new();

    let cfg = WalkConfig {
        hidden: false,
        git_ignore: true,
        git_global: true,
        git_exclude: true,
        ignore: true,
        parents: true,
    };

    let walker = WalkBuilder::new(scope)
        .follow_links(true)
        .hidden(cfg.hidden)
        .git_ignore(cfg.git_ignore)
        .git_global(cfg.git_global)
        .git_exclude(cfg.git_exclude)
        .ignore(cfg.ignore)
        .parents(cfg.parents)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                if let Some(name) = entry.file_name().to_str() {
                    return !crate::search::io::SKIP_DIRS.contains(&name);
                }
            }
            true
        })
        .max_depth(Some(depth + 1))
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let rel = path.strip_prefix(scope).unwrap_or(path);

        // Skip if deeper than requested
        let file_depth = rel.components().count().saturating_sub(1);
        if file_depth > depth {
            continue;
        }

        let parent = rel.parent().unwrap_or(Path::new("")).to_path_buf();
        let name = rel
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        let meta = std::fs::metadata(path).ok();
        let byte_len = meta.as_ref().map_or(0, std::fs::Metadata::len);
        let tokens = estimate_tokens(byte_len);

        let symbols = if include_symbols {
            let file_type = detect_file_type(path);
            match file_type {
                FileType::Code(_) => {
                    let mtime = meta
                        .and_then(|m| m.modified().ok())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

                    let outline_str = cache.get_or_compute(path, mtime, || {
                        let content = std::fs::read_to_string(path).unwrap_or_default();
                        let buf = content.as_bytes();
                        outline::generate(path, file_type, &content, buf, true)
                    });

                    Some(extract_symbol_names(&outline_str))
                }
                _ => None,
            }
        } else {
            None
        };

        tree.entry(parent.clone()).or_default().push(FileEntry {
            name,
            symbols,
            tokens,
        });

        // Ensure all ancestor directories exist in the tree so format_tree can find them.
        let mut ancestor = parent.parent();
        while let Some(a) = ancestor {
            tree.entry(a.to_path_buf()).or_default();
            if a == Path::new("") {
                break;
            }
            ancestor = a.parent();
        }
    }

    let mut out = format!(
        "# Map: {} (depth {}, sizes ~= tokens)\n",
        scope.display(),
        depth
    );
    out.push_str(&format_walk_note(&cfg));
    let totals = compute_dir_totals(&tree);
    format_tree(&tree, &totals, Path::new(""), 0, &mut out);

    let mut out = match budget {
        Some(b) => crate::budget::apply(&out, b),
        None => out,
    };
    if include_symbols {
        out.push_str("\n\n> Tip: narrow with --scope <dir>.\n");
    } else {
        out.push_str("\n\n> Tip: add --symbols, or narrow with --scope <dir>.\n");
    }
    out
}

/// Compute total tokens for each directory (sum of all descendant files).
fn compute_dir_totals(tree: &BTreeMap<PathBuf, Vec<FileEntry>>) -> BTreeMap<PathBuf, u64> {
    let mut totals: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for (dir, files) in tree {
        let sum: u64 = files.iter().map(|f| f.tokens).sum();
        // Add this dir's direct file tokens to itself and every ancestor.
        let mut cur: Option<&Path> = Some(dir.as_path());
        while let Some(p) = cur {
            *totals.entry(p.to_path_buf()).or_insert(0) += sum;
            if p == Path::new("") {
                break;
            }
            cur = p.parent();
        }
    }
    totals
}

struct FileEntry {
    name: String,
    symbols: Option<Vec<String>>,
    tokens: u64,
}

/// Extract symbol names from an outline string.
/// Outline lines look like: `[7-57]       fn classify`
/// We extract the last word(s) after the kind keyword.
fn extract_symbol_names(outline: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in outline.lines() {
        let trimmed = line.trim();
        // Skip import lines and empty lines
        if trimmed.starts_with('[') {
            // Find the symbol name after kind keywords
            if let Some(sig_start) = find_symbol_start(trimmed) {
                let sig = &trimmed[sig_start..];
                // Take just the name (up to first paren or space after name)
                let name = extract_name_from_sig(sig);
                if !name.is_empty() && name != "imports" {
                    names.push(name);
                }
            }
        }
    }
    names
}

fn find_symbol_start(line: &str) -> Option<usize> {
    let kinds = [
        "fn ",
        "struct ",
        "enum ",
        "trait ",
        "impl ",
        "mod ",
        "class ",
        "interface ",
        "type ",
        "const ",
        "static ",
        "function ",
        "method ",
        "def ",
    ];
    for kind in &kinds {
        if let Some(pos) = line.find(kind) {
            return Some(pos + kind.len());
        }
    }
    None
}

fn extract_name_from_sig(sig: &str) -> String {
    // Take characters until we hit a non-identifier char
    sig.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect()
}

fn format_tree(
    tree: &BTreeMap<PathBuf, Vec<FileEntry>>,
    totals: &BTreeMap<PathBuf, u64>,
    dir: &Path,
    indent: usize,
    out: &mut String,
) {
    // Collect subdirectories that have entries
    let mut subdirs: Vec<&PathBuf> = tree
        .keys()
        .filter(|k| k.parent() == Some(dir) && *k != dir)
        .collect();
    subdirs.sort();

    let prefix = "  ".repeat(indent);

    // Show files in this directory
    if let Some(files) = tree.get(dir) {
        for f in files {
            if let Some(ref symbols) = f.symbols {
                if symbols.is_empty() {
                    let _ = writeln!(out, "{prefix}{}  ~{}", f.name, fmt_tokens(f.tokens));
                } else {
                    let syms = symbols.join(", ");
                    let truncated = if syms.len() > 80 {
                        format!("{}...", crate::types::truncate_str(&syms, 77))
                    } else {
                        syms
                    };
                    let _ = writeln!(out, "{prefix}{}: {truncated}", f.name);
                }
            } else {
                let _ = writeln!(out, "{prefix}{}  ~{}", f.name, fmt_tokens(f.tokens));
            }
        }
    }

    // Recurse into subdirectories — show rollup token total next to dir name.
    for subdir in subdirs {
        let dir_name = subdir.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let total = totals.get(subdir).copied().unwrap_or(0);
        let _ = writeln!(out, "{prefix}{dir_name}/  ~{}", fmt_tokens(total));
        format_tree(tree, totals, subdir, indent + 1, out);
    }
}

/// Compact token count for directory rollups (e.g. "12.3k", "1.2M").
fn fmt_tokens(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)] // display-only; mantissa loss is fine for summaries
    let f = n as f64;
    if n >= 1_000_000 {
        format!("{:.1}M", f / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", f / 1_000.0)
    } else {
        n.to_string()
    }
}
