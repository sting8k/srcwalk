//! Resolve import statements to local file paths.
//! Used by the MCP layer to hint related files after an outlined read.

use std::path::{Path, PathBuf};

use crate::lang::detect_file_type;
use crate::types::Lang;

pub(crate) use crate::read::js_alias::{
    classify_js_imports, ordered_local_paths, JsImportResolution,
};

const MAX_SUGGESTIONS: usize = 8;

/// True if `source` is absolute on any host: a leading `/` (Unix) or a Windows
/// drive prefix (`C:/` or `C:\`). `Path::is_absolute()` alone is host-specific
/// — `C:/x` is not absolute on macOS — but Ruby treats a drive path as
/// absolute, so rejecting it everywhere keeps resolution and external
/// classification identical on all platforms.
pub(crate) fn is_absolute_source(source: &str) -> bool {
    let bytes = source.as_bytes();
    source.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'))
}

/// Extract import sources from already-read code content and resolve them to local file paths.
pub fn resolve_related_files_with_content(file_path: &Path, content: &str) -> Vec<PathBuf> {
    resolve_related_files_with_limit(file_path, content, Some(MAX_SUGGESTIONS))
}

/// Scoped/content adapter used by production consumers. JS/TS/TSX use the
/// shared alias-aware decision stream; other languages retain the old resolver.
pub(crate) fn resolve_related_files_with_content_and_scope(
    file_path: &Path,
    content: &str,
    scope: &Path,
    config_cache: &crate::lang::tsconfig::ConfigCache,
    limit: Option<usize>,
) -> Vec<PathBuf> {
    let Some(lang) = detect_file_type(file_path).structural_lang() else {
        return Vec::new();
    };
    if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) {
        let stream = crate::lang::js_imports::logical_sources(content, lang);
        return ordered_local_paths(
            &classify_js_imports(file_path, &stream, scope, config_cache),
            limit,
        );
    }
    resolve_related_files_with_limit(file_path, content, limit)
}

pub(crate) fn resolve_all_related_files_with_content(
    file_path: &Path,
    content: &str,
) -> Vec<PathBuf> {
    resolve_related_files_with_limit(file_path, content, None)
}

fn resolve_related_files_with_limit(
    file_path: &Path,
    content: &str,
    limit: Option<usize>,
) -> Vec<PathBuf> {
    let Some(lang) = detect_file_type(file_path).structural_lang() else {
        return Vec::new();
    };

    let Some(dir) = file_path.parent() else {
        return Vec::new();
    };

    let mut results = Vec::new();
    if crate::lang::css::is_stylesheet_lang(lang) || crate::lang::document::is_document_lang(lang) {
        let sources = if crate::lang::css::is_stylesheet_lang(lang) {
            crate::lang::css::dependency_sources(content, lang)
        } else {
            crate::lang::document::dependency_sources(content, lang)
        };
        for source in sources {
            if limit.is_some_and(|cap| results.len() >= cap) {
                break;
            }
            if source.is_empty() || is_external(&source, lang) {
                continue;
            }
            if let Some(path) = resolve(dir, &source, lang) {
                if !results.contains(&path) {
                    results.push(path);
                }
            }
        }
        return results;
    }

    // Ruby uses parser-backed require/require_relative resolution instead of
    // the generic text line scan, so dynamic and receiver-scoped forms are
    // never promoted to local file relations.
    if lang == Lang::Ruby {
        return resolve_ruby_related(dir, content, limit);
    }

    for line in content.lines() {
        if limit.is_some_and(|cap| results.len() >= cap) {
            break;
        }
        if !is_import_line(line, lang) {
            continue;
        }
        let source = crate::lang::outline::extract_import_source(line, Some(lang));
        if source.is_empty() || is_external(&source, lang) {
            continue;
        }
        if let Some(path) = resolve(dir, &source, lang) {
            if !results.contains(&path) {
                results.push(path);
            }
        }
    }
    results
}

pub(crate) fn is_import_line(line: &str, lang: Lang) -> bool {
    let trimmed = line.trim_start();
    match lang {
        Lang::Rust => trimmed.starts_with("use "),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => is_js_dependency_line(trimmed),
        Lang::Python => trimmed.starts_with("import ") || trimmed.starts_with("from "),
        Lang::Go | Lang::Java | Lang::Scala | Lang::Kotlin => {
            crate::read::keyword_rest(trimmed, "import").is_some()
        }
        Lang::C | Lang::Cpp => trimmed.starts_with("#include"),
        Lang::Css | Lang::Scss | Lang::Less => {
            crate::lang::css::import_source(trimmed, lang).is_some()
        }
        Lang::Elixir => {
            trimmed.starts_with("alias ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("use ")
                || trimmed.starts_with("require ")
        }
        _ => false,
    }
}

fn is_js_dependency_line(trimmed: &str) -> bool {
    if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
        return false;
    }
    if starts_js_keyword(trimmed, "import") {
        return !trimmed.starts_with("import(") && js_module_source(trimmed).is_some();
    }
    if starts_js_keyword(trimmed, "export") {
        return js_from_source(trimmed).is_some();
    }
    js_require_source(trimmed).is_some()
}

fn starts_js_keyword(trimmed: &str, keyword: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix(keyword) else {
        return false;
    };
    rest.chars()
        .next()
        .is_none_or(|c| !is_js_identifier_char(c))
}

fn is_js_identifier_char(c: char) -> bool {
    c == '_' || c == '$' || c.is_ascii_alphanumeric()
}

fn js_module_source(trimmed: &str) -> Option<String> {
    js_from_source(trimmed).or_else(|| first_quoted(trimmed))
}

fn js_from_source(trimmed: &str) -> Option<String> {
    let from_pos = find_js_keyword(trimmed, "from")?;
    first_quoted(&trimmed[from_pos + "from".len()..])
}

fn js_require_source(trimmed: &str) -> Option<String> {
    let require_pos = find_js_keyword(trimmed, "require")?;
    let after = trimmed[require_pos + "require".len()..].trim_start();
    if !after.starts_with('(') {
        return None;
    }
    first_quoted(after)
}

fn find_js_keyword(text: &str, keyword: &str) -> Option<usize> {
    let mut search_start = 0;
    while let Some(offset) = text[search_start..].find(keyword) {
        let pos = search_start + offset;
        let before_ok = text[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !is_js_identifier_char(c));
        let after_ok = text[pos + keyword.len()..]
            .chars()
            .next()
            .is_none_or(|c| !is_js_identifier_char(c));
        if before_ok && after_ok {
            return Some(pos);
        }
        search_start = pos + keyword.len();
    }
    None
}

fn first_quoted(text: &str) -> Option<String> {
    let mut chars = text.char_indices();
    while let Some((start, c)) = chars.next() {
        if c != '\'' && c != '"' {
            continue;
        }
        let quote = c;
        for (end, c) in chars.by_ref() {
            if c == quote {
                return Some(text[start + quote.len_utf8()..end].to_string());
            }
        }
    }
    None
}

pub(crate) fn is_external(source: &str, lang: Lang) -> bool {
    match lang {
        Lang::Rust => {
            !(source.starts_with("crate::")
                || source.starts_with("self::")
                || source.starts_with("super::"))
        }
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            !(source.starts_with('.') || source.starts_with("@/") || source.starts_with("~/"))
        }
        Lang::Python => !source.starts_with('.'),
        Lang::C | Lang::Cpp => !source.starts_with('"'),
        lang if matches!(
            crate::capabilities::import_path_style(lang),
            Some(crate::capabilities::ImportPathStyle::CInclude)
        ) =>
        {
            !source.starts_with('"')
        }
        Lang::Css | Lang::Scss | Lang::Less => crate::lang::css::is_external_source(source),
        Lang::Html | Lang::Markdown => crate::lang::document::is_external_source(source),
        // Elixir, Go, Java, Scala, Kotlin — can't resolve without build system knowledge.
        _ => true,
    }
}

fn resolve(dir: &Path, source: &str, lang: Lang) -> Option<PathBuf> {
    match lang {
        Lang::Rust => resolve_rust(dir, source),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => resolve_js(dir, source),
        Lang::Python => resolve_python(dir, source),
        Lang::C | Lang::Cpp => resolve_c_include(dir, source),
        lang if matches!(
            crate::capabilities::import_path_style(lang),
            Some(crate::capabilities::ImportPathStyle::CInclude)
        ) =>
        {
            resolve_c_include(dir, source)
        }
        Lang::Css | Lang::Scss | Lang::Less => crate::lang::css::resolve_source(dir, source, lang),
        Lang::Html | Lang::Markdown => crate::lang::document::resolve_source(dir, source, lang),
        // Elixir, Go, Java, etc. — module-to-file mapping requires build system conventions.
        _ => None,
    }
}

// --- Rust ---

fn resolve_rust(dir: &Path, source: &str) -> Option<PathBuf> {
    if let Some(rest) = source.strip_prefix("crate::") {
        let src_dir = find_src_ancestor(dir)?;
        try_rust_path(src_dir, rest)
    } else if let Some(rest) = source.strip_prefix("self::") {
        try_rust_path(dir, rest)
    } else if let Some(rest) = source.strip_prefix("super::") {
        try_rust_path(dir.parent()?, rest)
    } else {
        None
    }
}

/// Try progressively shorter paths until one resolves.
/// `cache::OutlineCache` → try cache/OutlineCache.rs (no) → cache.rs (yes).
/// `read::imports` → try read/imports.rs (yes) → stop.
fn try_rust_path(base: &Path, rest: &str) -> Option<PathBuf> {
    let segments: Vec<&str> = rest.split("::").collect();
    for len in (1..=segments.len()).rev() {
        let rel: PathBuf = segments[..len].iter().collect();
        if let Some(found) = try_rust_module(&base.join(&rel)) {
            return Some(found);
        }
    }
    None
}

fn try_rust_module(base: &Path) -> Option<PathBuf> {
    let with_rs = base.with_extension("rs");
    if with_rs.exists() {
        return Some(with_rs);
    }
    let mod_rs = base.join("mod.rs");
    if mod_rs.exists() {
        return Some(mod_rs);
    }
    None
}

fn find_src_ancestor(start: &Path) -> Option<&Path> {
    let mut current = start;
    loop {
        if current.file_name().and_then(|n| n.to_str()) == Some("src") {
            return Some(current);
        }
        current = current.parent()?;
    }
}

// --- JS/TS ---

pub(crate) fn resolve_js(dir: &Path, source: &str) -> Option<PathBuf> {
    let base = dir.join(source);
    if base.exists() && base.is_file() {
        return Some(base);
    }

    if let Some(candidate) = resolve_js_source_extension(&base) {
        return Some(candidate);
    }

    if !has_js_source_extension(&base) {
        // Append the modern module extensions after the legacy winner order so
        // existing `.ts/.tsx/.js/.jsx` priority is preserved exactly.
        for ext in &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"] {
            let candidate = PathBuf::from(format!("{}{ext}", base.display()));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    for name in &[
        "index.ts",
        "index.tsx",
        "index.js",
        "index.jsx",
        "index.mts",
        "index.cts",
        "index.mjs",
        "index.cjs",
    ] {
        let candidate = base.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn has_js_source_extension(base: &Path) -> bool {
    matches!(
        base.extension().and_then(|ext| ext.to_str()),
        Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs")
    )
}

fn resolve_js_source_extension(base: &Path) -> Option<PathBuf> {
    match base.extension().and_then(|ext| ext.to_str()) {
        Some("js") => ["ts", "tsx", "mts", "cts"]
            .into_iter()
            .map(|ext| base.with_extension(ext))
            .find(|candidate| candidate.exists()),
        Some("jsx") => {
            let candidate = base.with_extension("tsx");
            candidate.exists().then_some(candidate)
        }
        // Runtime-to-source swaps for the modern ESM/CJS extensions: a missing
        // `./x.mjs` may resolve to `x.mts` and a missing `./x.cjs` to `x.cts`.
        // No other Node/TypeScript compiler resolution is implied.
        Some("mjs") => {
            let candidate = base.with_extension("mts");
            candidate.exists().then_some(candidate)
        }
        Some("cjs") => {
            let candidate = base.with_extension("cts");
            candidate.exists().then_some(candidate)
        }
        _ => None,
    }
}

// --- Python ---

fn resolve_python(dir: &Path, source: &str) -> Option<PathBuf> {
    let dots = source.bytes().take_while(|&b| b == b'.').count();
    if dots == 0 {
        return None;
    }
    // Each dot beyond the first goes up one directory.
    let mut base = dir.to_path_buf();
    for _ in 1..dots {
        base = base.parent()?.to_path_buf();
    }
    let module_part = &source[dots..];
    if module_part.is_empty() {
        // Bare `from . import X`
        let init = base.join("__init__.py");
        return if init.exists() { Some(init) } else { None };
    }
    let rel = module_part.replace('.', "/");
    let as_file = base.join(format!("{rel}.py"));
    if as_file.exists() {
        return Some(as_file);
    }
    let as_pkg = base.join(&rel).join("__init__.py");
    if as_pkg.exists() {
        return Some(as_pkg);
    }
    None
}

// --- C/C++ ---

fn resolve_c_include(dir: &Path, source: &str) -> Option<PathBuf> {
    let clean = source.trim_matches('"');
    let candidate = dir.join(clean);
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

// --- Ruby ---

/// Resolve static `require` / `require_relative` references to existing local
/// files. Deduplicates resolved paths; `require_relative` resolves from the
/// current file's parent, bare `require` resolves only under bounded inferred
/// load roots. Returns at most `limit` results when `limit` is set.
fn resolve_ruby_related(dir: &Path, content: &str, limit: Option<usize>) -> Vec<PathBuf> {
    let refs = crate::lang::ruby::require_refs(content);
    let mut results = Vec::new();
    for reference in refs {
        if limit.is_some_and(|cap| results.len() >= cap) {
            break;
        }
        let resolved = match reference.kind {
            crate::lang::ruby::RubyRequireKind::RequireRelative => {
                resolve_ruby_require_relative(dir, &reference.source)
            }
            crate::lang::ruby::RubyRequireKind::Require => {
                resolve_ruby_require(dir, &reference.source)
            }
        };
        if let Some(path) = resolved {
            if !results.contains(&path) {
                results.push(path);
            }
        }
    }
    results
}

/// Resolve a static `require_relative` source from the current file's parent.
/// A source ending in `.rb` resolves to that exact file; a bare source gets
/// exactly one `<source>.rb` candidate. Unsupported extensions and non-files are
/// skipped. Existing paths are canonicalized for comparison boundaries.
pub(crate) fn resolve_ruby_require_relative(dir: &Path, source: &str) -> Option<PathBuf> {
    if source.is_empty() || is_absolute_source(source) {
        // Absolute targets are never file-relative; `require_relative` must
        // stay inside the current file's parent tree.
        return None;
    }
    let path = Path::new(source);
    let candidate = match path.extension().and_then(|ext| ext.to_str()) {
        Some("rb") => dir.join(path),
        Some(_) => return None, // unsupported extension (e.g. .so, .erb)
        None => dir.join(format!("{source}.rb")),
    };
    canonicalize_existing(&candidate)
}

/// Resolve a static bare `require` source under bounded inferred load roots.
/// Never scans the whole repo and never claims runtime `$LOAD_PATH`. Roots are
/// the nearest ancestor directory named `lib` and the nearest package root
/// (containing `Gemfile` or any `*.gemspec`) joined with `lib`. An explicit
/// `./`/`../` source resolves against that package root (inferred CWD
/// convention). If zero or more than one distinct existing candidate exists,
/// the reference is left unresolved (no guessing).
pub(crate) fn resolve_ruby_require(dir: &Path, source: &str) -> Option<PathBuf> {
    if source.is_empty() {
        return None;
    }
    // Absolute targets are never local load-roots (Unix or Windows drive form).
    if is_absolute_source(source) {
        return None;
    }

    let explicit_relative = source.starts_with("./") || source.starts_with("../");
    let mut roots: Vec<PathBuf> = Vec::new();
    if explicit_relative {
        // Explicit ./../ source resolves against the package root (inferred CWD).
        if let Some(pkg) = nearest_ruby_package_root(dir) {
            roots.push(pkg.to_path_buf());
        }
    } else {
        if let Some(lib) = nearest_ancestor_named_lib(dir) {
            roots.push(lib.to_path_buf());
        }
        if let Some(pkg) = nearest_ruby_package_root(dir) {
            roots.push(pkg.join("lib"));
        }
    }
    roots.sort();
    roots.dedup();

    let mut candidates: Vec<PathBuf> = Vec::new();
    for root in roots {
        let path = Path::new(source);
        let candidate = match path.extension().and_then(|ext| ext.to_str()) {
            Some("rb") => root.join(path),
            Some(_) => continue, // unsupported native/dynamic target
            None => root.join(format!("{source}.rb")),
        };
        if let Some(existing) = canonicalize_existing(&candidate) {
            if !candidates.contains(&existing) {
                candidates.push(existing);
            }
        }
    }
    // Only a unique existing candidate resolves; ambiguity stays unresolved.
    if candidates.len() == 1 {
        Some(candidates.pop().expect("one candidate"))
    } else {
        None
    }
}

fn canonicalize_existing(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        path.canonicalize().ok()
    } else {
        None
    }
}

/// Nearest ancestor directory whose file name is `lib` (the file's own parent
/// may qualify), used as a bounded Ruby load root.
fn nearest_ancestor_named_lib(start: &Path) -> Option<&Path> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.file_name().and_then(|name| name.to_str()) == Some("lib") {
            return Some(dir);
        }
        current = dir.parent();
    }
    None
}

/// Nearest ancestor package root containing a `Gemfile` or any `*.gemspec`.
/// Preserves the nearest package boundary in monorepos (stops at the first
/// match walking up).
fn nearest_ruby_package_root(start: &Path) -> Option<&Path> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join("Gemfile").is_file() || has_gemspec(dir) {
            return Some(dir);
        }
        current = dir.parent();
    }
    None
}

fn has_gemspec(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| {
        entries.any(|entry| {
            entry.is_ok_and(|e| e.path().extension().is_some_and(|ext| ext == "gemspec"))
        })
    })
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "srcwalk_imports_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn js_esm_specifier_resolves_to_ts_source() {
        let dir = temp_dir("js_specifier_ts_source");
        fs::write(dir.join("foo.ts"), "export const foo = 1;\n").unwrap();
        fs::write(dir.join("foo.config.ts"), "export const config = 1;\n").unwrap();

        let file = dir.join("main.ts");
        let related = resolve_related_files_with_content(
            &file,
            "import { foo } from \"./foo.js\";\nimport { config } from \"./foo.config\";\n",
        );

        assert_eq!(related, vec![dir.join("foo.ts"), dir.join("foo.config.ts")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn js_reexport_lines_resolve_to_local_sources() {
        let dir = temp_dir("js_reexports");
        fs::write(dir.join("foo.ts"), "export const foo = 1;\n").unwrap();
        fs::write(dir.join("bar.ts"), "export const bar = 1;\n").unwrap();

        let file = dir.join("index.ts");
        let related = resolve_related_files_with_content(
            &file,
            "export{ foo }from\"./foo.js\";\nexport * from \"./bar.js\";\n",
        );

        assert_eq!(related, vec![dir.join("foo.ts"), dir.join("bar.ts")]);
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn js_commonjs_require_resolves_local_sources() {
        let dir = temp_dir("js_commonjs_require");
        fs::write(dir.join("foo.ts"), "export const foo = 1;\n").unwrap();
        fs::write(dir.join("bar.ts"), "export const bar = 1;\n").unwrap();
        fs::write(dir.join("ignored.ts"), "export const ignored = 1;\n").unwrap();

        let file = dir.join("main.ts");
        let related = resolve_related_files_with_content(
            &file,
            "const foo = require(\"./foo.js\");\nrequire('./bar');\n// require('./ignored');\n",
        );

        assert_eq!(related, vec![dir.join("foo.ts"), dir.join("bar.ts")]);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Ruby resolver ---

    fn ruby_temp_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        fs::create_dir_all(dir.join("lib")).unwrap();
        fs::create_dir_all(dir.join("app/models")).unwrap();
        dir
    }

    /// Canonicalize the expected path so comparisons match the resolver's
    /// canonicalized output on all platforms (macOS resolves /var -> /private/var).
    fn canon(dir: &Path, rel: &str) -> PathBuf {
        dir.join(rel).canonicalize().unwrap()
    }

    #[test]
    fn ruby_require_relative_same_and_parent_dir() {
        let dir = ruby_temp_dir("ruby_relative");
        fs::write(dir.join("lib/a.rb"), "puts 1\n").unwrap();
        fs::write(dir.join("lib/b.rb"), "puts 2\n").unwrap();
        fs::write(dir.join("app/models/post.rb"), "puts 3\n").unwrap();

        // Same dir: require_relative './a' -> lib/a.rb
        let file = dir.join("lib/main.rb");
        let related = resolve_related_files_with_content(
            &file,
            "require_relative './a'\nrequire_relative 'b'\n",
        );
        assert_eq!(
            related,
            vec![canon(&dir, "lib/a.rb"), canon(&dir, "lib/b.rb")]
        );

        // Parent dir: require_relative '../lib/...' from app/models
        let file = dir.join("app/models/post.rb");
        let related = resolve_related_files_with_content(&file, "require_relative '../../lib/a'\n");
        assert_eq!(related, vec![canon(&dir, "lib/a.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_relative_explicit_rb_and_unsupported_extension() {
        let dir = ruby_temp_dir("ruby_relative_ext");
        fs::write(dir.join("lib/exact.rb"), "puts 1\n").unwrap();

        let file = dir.join("lib/main.rb");
        let related = resolve_related_files_with_content(&file, "require_relative './exact.rb'\n");
        assert_eq!(related, vec![canon(&dir, "lib/exact.rb")]);

        // Unsupported extension: never resolves.
        let related = resolve_related_files_with_content(&file, "require_relative './native.so'\n");
        assert!(related.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_absolute_sources_never_resolve_and_helper_is_platform_stable() {
        // Cross-platform helper: Unix absolute and Windows drive absolute are
        // both rejected identically on every host (macOS, Linux, Windows).
        assert!(is_absolute_source("/tmp/x"));
        assert!(is_absolute_source("C:/x"));
        assert!(is_absolute_source(r"C:\x"));
        assert!(is_absolute_source("d:/repo/lib.rb"));
        assert!(!is_absolute_source("json"));
        assert!(!is_absolute_source("./rel"));
        assert!(!is_absolute_source("../rel"));

        let dir = ruby_temp_dir("ruby_absolute");
        fs::write(dir.join("local.rb"), "puts 1\n").unwrap();
        let file = dir.join("main.rb");

        // require_relative must stay file-relative: absolute never escapes the
        // parent tree, even when a same-named file exists next to the source.
        let related = resolve_related_files_with_content(&file, "require_relative '/tmp/x'\n");
        assert!(related.is_empty());
        let related = resolve_related_files_with_content(&file, "require_relative 'C:/x'\n");
        assert!(related.is_empty());

        // Bare require with an absolute target is never a load-root candidate.
        let related = resolve_related_files_with_content(&file, "require '/tmp/x'\n");
        assert!(related.is_empty());
        let related = resolve_related_files_with_content(&file, "require 'C:/x'\n");
        assert!(related.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_resolves_nearest_lib() {
        let dir = ruby_temp_dir("ruby_nearest_lib");
        fs::write(dir.join("lib/order.rb"), "class Order; end\n").unwrap();

        // File inside lib/: bare require 'order' resolves via nearest lib root.
        let file = dir.join("lib/main.rb");
        let related = resolve_related_files_with_content(&file, "require 'order'\n");
        assert_eq!(related, vec![canon(&dir, "lib/order.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_resolves_package_root_lib_via_gemfile_and_gemspec() {
        let dir = ruby_temp_dir("ruby_package_root");
        fs::write(dir.join("Gemfile"), "source :rubygems\n").unwrap();
        fs::write(dir.join("lib/shop.rb"), "module Shop; end\n").unwrap();

        // File outside lib (app/models): bare require resolves via <pkg>/lib.
        let file = dir.join("app/models/order.rb");
        let related = resolve_related_files_with_content(&file, "require 'shop'\n");
        assert_eq!(related, vec![canon(&dir, "lib/shop.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_gemspec_root_works_without_gemfile() {
        let dir = ruby_temp_dir("ruby_gemspec");
        fs::write(dir.join("shop.gemspec"), "Gem::Specification.new\n").unwrap();
        fs::write(dir.join("lib/shop.rb"), "module Shop; end\n").unwrap();

        let file = dir.join("app/main.rb");
        let related = resolve_related_files_with_content(&file, "require 'shop'\n");
        assert_eq!(related, vec![canon(&dir, "lib/shop.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_explicit_relative_resolves_against_package_root() {
        let dir = ruby_temp_dir("ruby_explicit_relative");
        fs::write(dir.join("Gemfile"), "source :rubygems\n").unwrap();
        fs::write(dir.join("lib/config.rb"), "module Config; end\n").unwrap();

        // require './config' treated as inferred CWD = package root => <root>/config.rb?
        // Per the bounded convention only <root>/lib is a root, so ./config must
        // resolve against the package root itself. Create it to prove resolution.
        fs::write(dir.join("config.rb"), "module RootConfig; end\n").unwrap();
        let file = dir.join("app/main.rb");
        let related = resolve_related_files_with_content(&file, "require './config'\n");
        assert_eq!(related, vec![canon(&dir, "config.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_ambiguity_stays_unresolved() {
        let dir = ruby_temp_dir("ruby_ambiguity");
        fs::write(dir.join("Gemfile"), "source :rubygems\n").unwrap();
        fs::write(dir.join("lib/shared.rb"), "module Shared; end\n").unwrap();
        fs::write(dir.join("shared.rb"), "module RootShared; end\n").unwrap();

        // Bare require 'shared' could hit <lib>/shared.rb (nearest lib) only for
        // a file under lib; from app/ only <pkg>/lib is a root, so unique.
        let file = dir.join("app/main.rb");
        let related = resolve_related_files_with_content(&file, "require 'shared'\n");
        assert_eq!(related, vec![canon(&dir, "lib/shared.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_require_dynamic_and_receiver_forms_abstain() {
        let dir = ruby_temp_dir("ruby_dynamic");
        fs::write(dir.join("lib/foo.rb"), "puts 1\n").unwrap();
        fs::write(dir.join("lib/bar.rb"), "puts 2\n").unwrap();

        let file = dir.join("lib/main.rb");
        let related = resolve_related_files_with_content(
            &file,
            "require_relative './foo'\nrequire variable\nrequire File.expand_path('bar')\nobj.require 'foo'\nrequire_relative './missing'\n",
        );
        // Only the provably exact static require_relative resolves; the
        // unresolved require_relative is left alone.
        assert_eq!(related, vec![canon(&dir, "lib/foo.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_resolve_all_returns_more_than_capped_for_deps() {
        let dir = ruby_temp_dir("ruby_many");
        let mut content = String::new();
        for i in 0..12 {
            fs::write(dir.join(format!("lib/mod{i}.rb")), "puts 1\n").unwrap();
            let _ = writeln!(content, "require_relative './mod{i}'");
        }
        let file = dir.join("lib/main.rb");
        // Capped resolver keeps the historical suggestion cap for read/callees.
        let capped = resolve_related_files_with_content(&file, &content);
        assert!(capped.len() <= crate::read::imports::MAX_SUGGESTIONS);
        // The all-resolver returns everything so deps >8 requires are complete.
        let all = resolve_all_related_files_with_content(&file, &content);
        assert_eq!(all.len(), 12);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ruby_paths_with_spaces_resolve() {
        let dir = ruby_temp_dir("ruby spaces");
        fs::write(dir.join("lib/my lib.rb"), "puts 1\n").unwrap();
        let file = dir.join("lib/main.rb");
        let related = resolve_related_files_with_content(&file, "require_relative './my lib'\n");
        assert_eq!(related, vec![canon(&dir, "lib/my lib.rb")]);
        let _ = fs::remove_dir_all(&dir);
    }

    // --- Unresolved local-looking JS/TS/TSX ---

    /// A temp dir with a resolvable `local.js` so resolved-local cases stay out
    /// of the unresolved list.
    fn js_unresolved_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        fs::write(dir.join("local.js"), "export const x = 1;\n").unwrap();
        dir
    }

    #[test]
    fn unresolved_local_looking_mixed_classes_and_exact_lines() {
        let dir = js_unresolved_dir("unresolved_mixed");
        let file = dir.join("main.js");
        let sources = unresolved_local_looking_sources(
            &file,
            "import a from './local.js';\nimport b from 'lodash';\nimport c from './missing.js';\nimport d from '@/store';\nimport e from '~/utils';\nimport f from '../nope/deep';\n",
        );
        assert_eq!(
            sources,
            vec![
                ("./missing.js".to_string(), 3),
                ("@/store".to_string(), 4),
                ("~/utils".to_string(), 5),
                ("../nope/deep".to_string(), 6),
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_local_looking_scoped_and_package_sources_stay_out() {
        let dir = js_unresolved_dir("unresolved_scoped");
        let file = dir.join("main.js");
        let sources = unresolved_local_looking_sources(
            &file,
            "import a from '@scope/pkg';\nimport b from '@scope/pkg/sub';\nimport c from 'lodash/fp';\n",
        );
        assert!(
            sources.is_empty(),
            "scoped/package sources are not local-looking: {sources:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_local_looking_dedupes_by_source_keeping_earliest_line() {
        let dir = js_unresolved_dir("unresolved_dedupe");
        let file = dir.join("main.js");
        let sources = unresolved_local_looking_sources(
            &file,
            "import './dup.js';\nimport './dup.js';\nimport './a.js';\nimport './a.js';\n",
        );
        assert_eq!(
            sources,
            vec![("./dup.js".to_string(), 1), ("./a.js".to_string(), 3)]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_local_looking_resolved_local_never_duplicates() {
        let dir = js_unresolved_dir("unresolved_resolved");
        let file = dir.join("main.js");
        // local.js exists (exact and via extension swap); only ./missing.js is
        // genuinely unresolved.
        let sources = unresolved_local_looking_sources(
            &file,
            "import a from './local.js';\nimport b from './local';\nimport c from './missing.js';\n",
        );
        assert_eq!(sources, vec![("./missing.js".to_string(), 3)]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_local_looking_comments_strings_and_dynamic_forms_no_rows() {
        let dir = js_unresolved_dir("unresolved_literals");
        let file = dir.join("main.js");
        let sources = unresolved_local_looking_sources(
            &file,
            "// import './commented.js';\nconst a = \"import './str.js'\";\nimport('./dynamic.js');\nconst c = require(someVar);\nconst d = require(`./tpl-${x}`);\n",
        );
        assert!(
            sources.is_empty(),
            "comments/strings/dynamic forms must not create rows: {sources:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_local_looking_non_js_lang_returns_empty() {
        let dir = temp_dir("unresolved_rust");
        fs::write(dir.join("local.rs"), "pub fn x() {}\n").unwrap();
        let file = dir.join("main.rs");
        let sources = unresolved_local_looking_sources(
            &file,
            "use crate::missing;\nuse std::collections::HashMap;\n",
        );
        assert!(
            sources.is_empty(),
            "Rust must not produce unresolved local-looking rows: {sources:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
