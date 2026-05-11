//! Resolve import statements to local file paths.
//! Used by the MCP layer to hint related files after an outlined read.

use std::fs;
use std::path::{Path, PathBuf};

use crate::lang::detect_file_type;
use crate::types::{FileType, Lang};

const MAX_SUGGESTIONS: usize = 8;

/// Extract import sources from a code file and resolve them to existing local file paths.
/// Returns empty Vec for non-code files, files with no imports, or when all imports are external.
pub fn resolve_related_files(file_path: &Path) -> Vec<PathBuf> {
    let Ok(content) = fs::read_to_string(file_path) else {
        return Vec::new();
    };
    resolve_related_files_with_content(file_path, &content)
}

/// Same as `resolve_related_files` but takes pre-read content to avoid a redundant file read.
pub fn resolve_related_files_with_content(file_path: &Path, content: &str) -> Vec<PathBuf> {
    resolve_related_files_with_limit(file_path, content, Some(MAX_SUGGESTIONS))
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
    let FileType::Code(lang) = detect_file_type(file_path) else {
        return Vec::new();
    };

    let Some(dir) = file_path.parent() else {
        return Vec::new();
    };

    let mut results = Vec::new();
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
        Lang::Go | Lang::Java | Lang::Scala | Lang::Kotlin => trimmed.starts_with("import "),
        Lang::C | Lang::Cpp => trimmed.starts_with("#include"),
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

fn resolve_js(dir: &Path, source: &str) -> Option<PathBuf> {
    let base = dir.join(source);
    if base.exists() && base.is_file() {
        return Some(base);
    }

    if let Some(candidate) = resolve_js_source_extension(&base) {
        return Some(candidate);
    }

    if !has_js_source_extension(&base) {
        for ext in &[".ts", ".tsx", ".js", ".jsx"] {
            let candidate = PathBuf::from(format!("{}{ext}", base.display()));
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    for name in &["index.ts", "index.tsx", "index.js", "index.jsx"] {
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
        Some("ts" | "tsx" | "js" | "jsx")
    )
}

fn resolve_js_source_extension(base: &Path) -> Option<PathBuf> {
    match base.extension().and_then(|ext| ext.to_str()) {
        Some("js") => ["ts", "tsx"]
            .into_iter()
            .map(|ext| base.with_extension(ext))
            .find(|candidate| candidate.exists()),
        Some("jsx") => {
            let candidate = base.with_extension("tsx");
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "srcwalk_imports_{name}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
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
}
