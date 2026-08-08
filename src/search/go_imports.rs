use std::fs;
use std::path::Path;

pub(super) fn is_stdlib(path: &Path, source: &str) -> bool {
    if source.contains('.') {
        return false;
    }
    if find_go_module(path).is_some_and(|module| source_matches_module(&module, source)) {
        return false;
    }
    known_stdlib(source)
}

fn find_go_module(path: &Path) -> Option<String> {
    let start = path.parent()?;
    for dir in start.ancestors() {
        let Ok(content) = fs::read_to_string(dir.join("go.mod")) else {
            continue;
        };
        if let Some(module) = content.lines().find_map(module_directive) {
            return Some(module.to_owned());
        }
    }
    None
}

/// Parse the module directive by token boundaries, not one literal space.
fn module_directive(line: &str) -> Option<&str> {
    let rest = line.trim().strip_prefix("module")?;
    if !rest.chars().next()?.is_whitespace() {
        return None;
    }
    let module = rest.split_whitespace().next()?.trim_matches('"');
    (!module.is_empty()).then_some(module)
}

fn source_matches_module(module: &str, source: &str) -> bool {
    source == module
        || source
            .strip_prefix(module)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn known_stdlib(source: &str) -> bool {
    const ROOTS: &[&str] = &[
        "archive",
        "bufio",
        "bytes",
        "cmp",
        "compress",
        "container",
        "context",
        "crypto",
        "database",
        "debug",
        "embed",
        "encoding",
        "errors",
        "expvar",
        "flag",
        "fmt",
        "go",
        "hash",
        "html",
        "image",
        "index",
        "io",
        "iter",
        "log",
        "maps",
        "math",
        "mime",
        "net",
        "os",
        "path",
        "plugin",
        "reflect",
        "regexp",
        "runtime",
        "slices",
        "sort",
        "strconv",
        "strings",
        "sync",
        "syscall",
        "testing",
        "text",
        "time",
        "unicode",
        "unsafe",
    ];
    ROOTS.iter().any(|root| {
        source == *root
            || source
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::{known_stdlib, module_directive, source_matches_module};

    #[test]
    fn module_directive_accepts_token_whitespace_and_quotes() {
        assert_eq!(module_directive("module myapp"), Some("myapp"));
        assert_eq!(module_directive("module\tmyapp // comment"), Some("myapp"));
        assert_eq!(
            module_directive("  module   \"example.com/app\""),
            Some("example.com/app")
        );
        assert_eq!(module_directive("modulex/myapp"), None);
        assert_eq!(module_directive("module"), None);
    }

    #[test]
    fn module_match_requires_a_path_boundary() {
        assert!(source_matches_module("myapp", "myapp"));
        assert!(source_matches_module("myapp", "myapp/internal/config"));
        assert!(!source_matches_module("myapp", "myapplication/config"));
    }

    #[test]
    fn missing_manifest_fallback_only_omits_known_stdlib() {
        assert!(known_stdlib("fmt"));
        assert!(known_stdlib("net/http"));
        assert!(!known_stdlib("fmtx"));
        assert!(!known_stdlib("myapp/internal/config"));
    }
}
