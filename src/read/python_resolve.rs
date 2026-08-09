use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolve Python imports under bounded project roots. A source resolves only
/// when exactly one file/package candidate exists across those roots.
pub(crate) fn resolve(file_dir: &Path, source: &str, scope: Option<&Path>) -> Option<PathBuf> {
    let roots = resolution_roots(file_dir, source, scope)?;
    unique_candidates(&roots, source, scope)
}

/// True when more than one bounded file/package candidate exists.
pub(crate) fn is_ambiguous(file_dir: &Path, source: &str, scope: Option<&Path>) -> bool {
    resolution_roots(file_dir, source, scope)
        .is_some_and(|roots| candidate_paths(&roots, source, scope).len() > 1)
}

fn resolution_roots(file_dir: &Path, source: &str, scope: Option<&Path>) -> Option<Vec<PathBuf>> {
    let dots = source.bytes().take_while(|&byte| byte == b'.').count();
    if dots > 0 {
        let mut base = file_dir.to_path_buf();
        for _ in 1..dots {
            base = base.parent()?.to_path_buf();
        }
        return Some(vec![base]);
    }
    let mut roots = project_roots(file_dir, scope);
    roots.sort();
    roots.dedup();
    Some(roots)
}

fn unique_candidates(roots: &[PathBuf], module: &str, scope: Option<&Path>) -> Option<PathBuf> {
    let mut candidates = candidate_paths(roots, module, scope);
    (candidates.len() == 1)
        .then(|| candidates.drain().next())
        .flatten()
}

fn candidate_paths(roots: &[PathBuf], module: &str, scope: Option<&Path>) -> HashSet<PathBuf> {
    let module = module.trim_matches('.');
    if module.is_empty() || module.contains('/') || module.contains('\\') {
        return HashSet::new();
    }
    let rel = module.replace('.', "/");
    let mut candidates = HashSet::new();
    for root in roots {
        let file = root.join(format!("{rel}.py"));
        if let Some(path) = existing_file(&file, scope) {
            candidates.insert(path);
        }
        let package = root.join(&rel).join("__init__.py");
        if let Some(path) = existing_file(&package, scope) {
            candidates.insert(path);
        }
    }
    candidates
}

fn project_roots(file_dir: &Path, scope: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(scope) = scope {
        roots.push(scope.to_path_buf());
        let src = scope.join("src");
        if src.is_dir() {
            roots.push(src);
        }
    }

    let mut current = Some(file_dir);
    while let Some(dir) = current {
        if dir.join("pyproject.toml").is_file() || dir.join("setup.py").is_file() {
            roots.push(dir.to_path_buf());
            let src = dir.join("src");
            if src.is_dir() {
                roots.push(src);
            }
            break;
        }
        current = dir.parent();
    }

    if roots.is_empty() {
        roots.push(file_dir.to_path_buf());
    }
    roots
}

fn existing_file(path: &Path, scope: Option<&Path>) -> Option<PathBuf> {
    let path = path
        .is_file()
        .then(|| path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))?;
    scope
        .is_none_or(|scope| crate::read::imports::path_within_scope(&path, scope))
        .then_some(path)
}
