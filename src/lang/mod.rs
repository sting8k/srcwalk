pub(crate) mod css;
pub(crate) mod decision_flow;
pub mod detection;
pub(crate) mod document;
pub(crate) mod js_imports;
pub mod outline;
pub(crate) mod qualified;
pub(crate) mod ruby;
pub(crate) mod scoped_occurrences;
pub mod treesitter;
pub(crate) mod tsconfig;

use std::path::Path;

use crate::types::{FileType, Lang};

/// Detect file type by extension, then by name.
pub fn detect_file_type(path: &Path) -> FileType {
    if let Some(file_type) = crate::capabilities::detect_file_type(path) {
        return file_type;
    }
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts" | "mts" | "cts") => FileType::Code(Lang::TypeScript),
        Some("tsx") => FileType::Code(Lang::Tsx),
        Some("js" | "jsx" | "mjs" | "cjs") => FileType::Code(Lang::JavaScript),
        Some("py" | "pyi") => FileType::Code(Lang::Python),
        Some("rs") => FileType::Code(Lang::Rust),
        Some("go") => FileType::Code(Lang::Go),
        Some("java") => FileType::Code(Lang::Java),
        Some("scala" | "sc") => FileType::Code(Lang::Scala),
        Some("c" | "h") => FileType::Code(Lang::C),
        Some("cpp" | "hpp" | "cc" | "cxx") => FileType::Code(Lang::Cpp),
        Some("rb") => FileType::Code(Lang::Ruby),
        Some("php" | "phtml") => FileType::Code(Lang::Php),
        Some("swift") => FileType::Code(Lang::Swift),
        Some("kt" | "kts") => FileType::Code(Lang::Kotlin),
        Some("cs") => FileType::Code(Lang::CSharp),
        Some("ex" | "exs") => FileType::Code(Lang::Elixir),
        Some("css") => FileType::Code(Lang::Css),
        Some("scss") => FileType::Code(Lang::Scss),
        Some("less") => FileType::Code(Lang::Less),
        Some("html" | "htm") => FileType::Document(Lang::Html),

        Some("md" | "mdx" | "rst") => FileType::Document(Lang::Markdown),
        Some("json" | "yaml" | "yml" | "toml" | "xml" | "ini") => FileType::StructuredData,
        Some("csv" | "tsv") => FileType::Tabular,
        Some("log") => FileType::Log,

        None => file_type_from_name(path),
        _ => FileType::Other,
    }
}

fn file_type_from_name(path: &Path) -> FileType {
    match path.file_name().and_then(|n| n.to_str()) {
        Some("Dockerfile" | "Containerfile") => FileType::Code(Lang::Dockerfile),
        Some("Makefile" | "GNUmakefile") => FileType::Code(Lang::Make),
        Some("Vagrantfile" | "Rakefile") => FileType::Code(Lang::Ruby),
        Some(n) if n.starts_with(".env") => FileType::StructuredData,
        _ => FileType::Other,
    }
}

/// Find the nearest package root by looking for manifest files.
pub(crate) fn package_root(path: &Path) -> Option<&Path> {
    const MANIFESTS: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.sbt",
        "mix.exs",
    ];
    let mut dir = path;
    loop {
        for m in MANIFESTS {
            if dir.join(m).exists() {
                return Some(dir);
            }
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(ext: &str) -> FileType {
        detect_file_type(Path::new(&format!("dir/file.{ext}")))
    }

    /// US-052 Phase 3 detection matrix: modern JS/TS module extensions enter
    /// the same structural tier as their language; gjs/gts stay unsupported.
    #[test]
    fn modern_js_ts_module_extensions_detect_to_language_tiers() {
        assert_eq!(
            detect("mjs"),
            FileType::Code(Lang::JavaScript),
            ".mjs must map to the JavaScript structural tier"
        );
        assert_eq!(
            detect("cjs"),
            FileType::Code(Lang::JavaScript),
            ".cjs must map to the JavaScript structural tier"
        );
        assert_eq!(
            detect("mts"),
            FileType::Code(Lang::TypeScript),
            ".mts must map to the TypeScript structural tier"
        );
        assert_eq!(
            detect("cts"),
            FileType::Code(Lang::TypeScript),
            ".cts must map to the TypeScript structural tier"
        );
        // Existing tiers unchanged.
        assert_eq!(detect("js"), FileType::Code(Lang::JavaScript));
        assert_eq!(detect("jsx"), FileType::Code(Lang::JavaScript));
        assert_eq!(detect("ts"), FileType::Code(Lang::TypeScript));
        assert_eq!(detect("tsx"), FileType::Code(Lang::Tsx));
    }

    /// Ember/Glimmer and other non-canonical JS-like extensions stay out of
    /// scope: no new language tier, no gjs/gts support.
    #[test]
    fn glimmer_extensions_remain_unsupported() {
        assert_eq!(detect("gjs"), FileType::Other);
        assert_eq!(detect("gts"), FileType::Other);
    }
}
