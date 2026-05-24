use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::Command;

use globset::Glob;

use crate::budget;
use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::evidence::{render_next_actions, Anchor, EvidenceSource, NextAction};
use crate::lang::detect_file_type;
use crate::lang::outline::{outline_language, walk_top_level};
use crate::types::{estimate_tokens, FileType, OutlineEntry, OutlineKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffMode {
    Working,
    Staged,
    Range,
}

#[derive(Clone, Debug)]
pub(crate) struct DiffFile {
    pub(crate) path: String,
    pub(crate) old_path: Option<String>,
    pub(crate) status: DiffStatus,
    pub(crate) hunks: Vec<DiffHunk>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DiffStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
}

impl DiffStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiffHunk {
    pub(crate) old_start: u32,
    pub(crate) old_lines: u32,
    pub(crate) new_start: u32,
    pub(crate) new_lines: u32,
    pub(crate) symbol: Option<EnclosingSymbol>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct EnclosingSymbol {
    pub(crate) name: String,
    pub(crate) kind: OutlineKind,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct DiffEvidence {
    pub(crate) repo_root: PathBuf,
    pub(crate) rev_range: Option<String>,
    pub(crate) mode: DiffMode,
    pub(crate) files: Vec<DiffFile>,
    pub(crate) total_files: usize,
    pub(crate) total_hunks: usize,
    pub(crate) total_symbols: usize,
}

impl DiffEvidence {
    pub(crate) fn title(&self) -> String {
        diff_title(self.rev_range.as_deref(), self.mode)
    }
}

struct ScopeFilter {
    repo_root: PathBuf,
    scope: PathBuf,
    scope_is_file: bool,
    glob: Option<globset::GlobMatcher>,
}

pub(crate) fn run_diff(
    rev_range: Option<&str>,
    mode: DiffMode,
    scope: &Path,
    scope_glob: Option<&str>,
    budget_tokens: Option<u64>,
    limit: Option<usize>,
    offset: usize,
    _cache: &OutlineCache,
) -> Result<String, SrcwalkError> {
    let evidence = collect_diff_evidence(rev_range, mode, scope, scope_glob)?;
    let page_size = limit.unwrap_or(20);
    let shown_files: Vec<&DiffFile> = evidence.files.iter().skip(offset).take(page_size).collect();

    let mut out = format_diff_result(
        evidence.rev_range.as_deref(),
        evidence.mode,
        &shown_files,
        evidence.total_files,
        evidence.total_hunks,
        evidence.total_symbols,
        limit,
        offset,
    );
    if let Some(budget) = budget_tokens {
        out = budget::apply_preserving_footer(&out, budget);
    }
    Ok(out)
}

pub(crate) fn collect_diff_evidence(
    rev_range: Option<&str>,
    mode: DiffMode,
    scope: &Path,
    scope_glob: Option<&str>,
) -> Result<DiffEvidence, SrcwalkError> {
    if mode == DiffMode::Staged && rev_range.is_some() {
        return Err(SrcwalkError::InvalidQuery {
            query: rev_range.unwrap_or_default().to_string(),
            reason: "--staged cannot be combined with a revision range".to_string(),
        });
    }
    if let Some(range) = rev_range {
        validate_rev_range(range)?;
    }

    let repo_root = git_repo_root()?;
    let mut files = match mode {
        DiffMode::Working => parse_git_patch(&git_output([
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--unified=0",
            "HEAD",
            "--",
        ])?)?,
        DiffMode::Staged => parse_git_patch(&git_output([
            "diff",
            "--cached",
            "--no-ext-diff",
            "--find-renames",
            "--unified=0",
            "--",
        ])?)?,
        DiffMode::Range => parse_git_patch(&git_output([
            "diff",
            "--no-ext-diff",
            "--find-renames",
            "--unified=0",
            rev_range.unwrap_or_default(),
            "--",
        ])?)?,
    };

    if mode == DiffMode::Working {
        files.extend(untracked_files(&repo_root)?);
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    let filter = ScopeFilter::new(repo_root.clone(), scope, scope_glob)?;
    files.retain(|file| filter.matches(&file.path));
    attach_symbols(&mut files, &repo_root, rev_range, mode);

    let total_files = files.len();
    let total_hunks = files.iter().map(|file| file.hunks.len()).sum::<usize>();
    let total_symbols = changed_symbols(&files).len();

    Ok(DiffEvidence {
        repo_root,
        rev_range: rev_range.map(str::to_string),
        mode,
        files,
        total_files,
        total_hunks,
        total_symbols,
    })
}

pub(crate) fn is_explicit_rev_range(range: &str) -> bool {
    range.contains("..") && !range.starts_with("..") && !range.ends_with("..")
}

fn validate_rev_range(range: &str) -> Result<(), SrcwalkError> {
    if is_explicit_rev_range(range) {
        return Ok(());
    }
    Err(SrcwalkError::InvalidQuery {
        query: range.to_string(),
        reason: "diff revision range must use explicit A..B or A...B; use REV^..REV for one commit"
            .to_string(),
    })
}

impl ScopeFilter {
    fn new(
        repo_root: PathBuf,
        scope: &Path,
        scope_glob: Option<&str>,
    ) -> Result<Self, SrcwalkError> {
        let glob = scope_glob
            .map(|pattern| {
                let normalized = pattern.trim_start_matches('/');
                Glob::new(normalized)
                    .or_else(|_| Glob::new(pattern))
                    .map_err(|e| SrcwalkError::InvalidQuery {
                        query: pattern.to_string(),
                        reason: e.to_string(),
                    })
                    .map(|glob| glob.compile_matcher())
            })
            .transpose()?;
        Ok(Self {
            repo_root: normalize_existing_path(repo_root),
            scope: normalize_existing_path(scope.to_path_buf()),
            scope_is_file: scope.is_file(),
            glob,
        })
    }

    fn matches(&self, git_path: &str) -> bool {
        let absolute = self.repo_root.join(git_path);
        if self.scope_is_file {
            return same_path(&absolute, &self.scope);
        }
        if !absolute.starts_with(&self.scope) {
            return false;
        }
        let Some(glob) = &self.glob else {
            return true;
        };
        let rel = absolute.strip_prefix(&self.scope).unwrap_or(&absolute);
        let name = absolute
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        glob.is_match(rel) || glob.is_match(name)
    }
}

fn normalize_existing_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn git_repo_root() -> Result<PathBuf, SrcwalkError> {
    let output = git_output(["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(output.trim()))
}

fn git_output<const N: usize>(args: [&str; N]) -> Result<String, SrcwalkError> {
    let output =
        Command::new("git")
            .args(args)
            .output()
            .map_err(|source| SrcwalkError::IoError {
                path: PathBuf::from("git"),
                source,
            })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(SrcwalkError::InvalidQuery {
            query: "diff".to_string(),
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn parse_git_patch(patch: &str) -> Result<Vec<DiffFile>, SrcwalkError> {
    let mut files = Vec::new();
    let mut current: Option<DiffFile> = None;

    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(file);
            }
            current = Some(parse_diff_git_line(rest));
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue;
        };
        if line.starts_with("new file mode ") {
            file.status = DiffStatus::Added;
        } else if line.starts_with("deleted file mode ") {
            file.status = DiffStatus::Deleted;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            file.old_path = Some(clean_patch_path(path));
            file.status = DiffStatus::Renamed;
        } else if let Some(path) = line.strip_prefix("rename to ") {
            file.path = clean_patch_path(path);
            file.status = DiffStatus::Renamed;
        } else if let Some(path) = line.strip_prefix("+++ ") {
            if let Some(path) = normalize_patch_path(path) {
                file.path = path;
            }
        } else if let Some(hunk) = parse_hunk_header(line) {
            file.hunks.push(hunk);
        }
    }

    if let Some(file) = current {
        files.push(file);
    }
    Ok(files)
}

fn parse_diff_git_line(rest: &str) -> DiffFile {
    let (old_path, path) = parse_diff_git_paths(rest)
        .or_else(|| {
            let mut parts = rest.split_whitespace();
            let old_path = parts.next().and_then(normalize_patch_path);
            let path = parts
                .next()
                .and_then(normalize_patch_path)
                .or_else(|| old_path.clone())?;
            Some((old_path, path))
        })
        .unwrap_or_else(|| (None, "?".to_string()));
    DiffFile {
        path,
        old_path,
        status: DiffStatus::Modified,
        hunks: Vec::new(),
    }
}

fn parse_diff_git_paths(rest: &str) -> Option<(Option<String>, String)> {
    let rest = rest.trim();
    if let Some(rest) = rest.strip_prefix("a/") {
        let (old_path, new_path) = rest.rsplit_once(" b/")?;
        return Some((Some(clean_patch_path(old_path)), clean_patch_path(new_path)));
    }
    if let Some(rest) = rest.strip_prefix("\"a/") {
        let (old_path, new_path) = rest.rsplit_once("\" \"b/")?;
        return Some((
            Some(clean_patch_path(old_path)),
            clean_patch_path(new_path.trim_end_matches('"')),
        ));
    }
    None
}

fn normalize_patch_path(path: &str) -> Option<String> {
    let path = path.trim();
    if path == "/dev/null" {
        return None;
    }
    path.strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .or_else(|| path.strip_prefix('"').and_then(|p| p.strip_suffix('"')))
        .map(clean_patch_path)
}

fn clean_patch_path(path: &str) -> String {
    path.trim().replace('\\', "/")
}

fn parse_hunk_header(line: &str) -> Option<DiffHunk> {
    let rest = line.strip_prefix("@@ ")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let mut parts = ranges.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let (old_start, old_lines) = parse_range(old)?;
    let (new_start, new_lines) = parse_range(new)?;
    Some(DiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        symbol: None,
    })
}

fn parse_range(range: &str) -> Option<(u32, u32)> {
    if let Some((start, lines)) = range.split_once(',') {
        Some((start.parse().ok()?, lines.parse().ok()?))
    } else {
        Some((range.parse().ok()?, 1))
    }
}

fn untracked_files(repo_root: &Path) -> Result<Vec<DiffFile>, SrcwalkError> {
    let output = git_output(["ls-files", "--others", "--exclude-standard"])?;
    let mut files = Vec::new();
    for path in output.lines().filter(|line| !line.trim().is_empty()) {
        let full_path = repo_root.join(path);
        let line_count = count_lines_cheap(&full_path).unwrap_or(0);
        files.push(DiffFile {
            path: path.replace('\\', "/"),
            old_path: None,
            status: DiffStatus::Added,
            hunks: vec![DiffHunk {
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: line_count,
                symbol: None,
            }],
        });
    }
    Ok(files)
}

fn count_lines_cheap(path: &Path) -> io::Result<u32> {
    let file = std::fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    Ok(reader.lines().count().min(u32::MAX as usize) as u32)
}

fn attach_symbols(
    files: &mut [DiffFile],
    repo_root: &Path,
    rev_range: Option<&str>,
    mode: DiffMode,
) {
    for file in files {
        if file.status == DiffStatus::Deleted {
            continue;
        }
        let Some(content) = after_content(repo_root, &file.path, rev_range, mode) else {
            continue;
        };
        let entries = outline_entries(&repo_root.join(&file.path), &content);
        for hunk in &mut file.hunks {
            let Some((start, end)) = symbol_anchor_range(hunk) else {
                continue;
            };
            hunk.symbol = enclosing_symbol(&entries, start, end);
        }
    }
}

fn symbol_anchor_range(hunk: &DiffHunk) -> Option<(u32, u32)> {
    if hunk.new_lines == 0 {
        (hunk.new_start > 0).then_some((hunk.new_start, hunk.new_start))
    } else {
        Some((hunk.new_start, hunk_end(hunk.new_start, hunk.new_lines)))
    }
}

pub(crate) fn after_content(
    repo_root: &Path,
    path: &str,
    rev_range: Option<&str>,
    mode: DiffMode,
) -> Option<String> {
    match mode {
        DiffMode::Working => std::fs::read_to_string(repo_root.join(path)).ok(),
        DiffMode::Staged => git_output(["show", &format!(":{path}")]).ok(),
        DiffMode::Range => {
            let rev = after_rev(rev_range?)?;
            git_output(["show", &format!("{rev}:{path}")]).ok()
        }
    }
}

fn after_rev(range: &str) -> Option<&str> {
    range
        .rsplit_once("...")
        .or_else(|| range.rsplit_once(".."))
        .map(|(_, right)| right)
        .filter(|right| !right.is_empty())
}

fn outline_entries(path: &Path, content: &str) -> Vec<OutlineEntry> {
    let FileType::Code(lang) = detect_file_type(path) else {
        return Vec::new();
    };
    if let Some(entries) = crate::capabilities::outline_entries(lang, content) {
        return entries;
    }
    let Some(language) = outline_language(lang) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    walk_top_level(tree.root_node(), &lines, lang)
}

fn enclosing_symbol(entries: &[OutlineEntry], start: u32, end: u32) -> Option<EnclosingSymbol> {
    let mut best = None;
    for entry in entries {
        collect_enclosing_symbol(entry, start, end, &mut best);
    }
    best
}

fn collect_enclosing_symbol(
    entry: &OutlineEntry,
    start: u32,
    end: u32,
    best: &mut Option<EnclosingSymbol>,
) {
    if entry.start_line <= start && end <= entry.end_line {
        if entry.kind == OutlineKind::Import {
            return;
        }
        let candidate = EnclosingSymbol {
            name: entry.name.clone(),
            kind: entry.kind,
            start_line: entry.start_line,
            end_line: entry.end_line,
        };
        let candidate_span = candidate.end_line.saturating_sub(candidate.start_line);
        let best_span = best.as_ref().map_or(u32::MAX, |symbol| {
            symbol.end_line.saturating_sub(symbol.start_line)
        });
        if candidate_span <= best_span {
            *best = Some(candidate);
        }
        for child in &entry.children {
            collect_enclosing_symbol(child, start, end, best);
        }
    }
}

fn changed_symbols(files: &[DiffFile]) -> HashSet<(String, EnclosingSymbol)> {
    let mut symbols = HashSet::new();
    for file in files {
        for hunk in &file.hunks {
            if let Some(symbol) = &hunk.symbol {
                symbols.insert((file.path.clone(), symbol.clone()));
            }
        }
    }
    symbols
}

fn diff_title(rev_range: Option<&str>, mode: DiffMode) -> String {
    match mode {
        DiffMode::Working => "working tree".to_string(),
        DiffMode::Staged => "staged".to_string(),
        DiffMode::Range => rev_range.unwrap_or_default().to_string(),
    }
}

fn format_diff_result(
    rev_range: Option<&str>,
    mode: DiffMode,
    files: &[&DiffFile],
    total_files: usize,
    total_hunks: usize,
    total_symbols: usize,
    limit: Option<usize>,
    offset: usize,
) -> String {
    let title = diff_title(rev_range, mode);
    let shown_hunks = files.iter().map(|file| file.hunks.len()).sum::<usize>();
    let shown_symbols = files
        .iter()
        .flat_map(|file| {
            file.hunks.iter().filter_map(|hunk| {
                hunk.symbol
                    .as_ref()
                    .map(|symbol| (file.path.clone(), symbol.clone()))
            })
        })
        .collect::<HashSet<_>>()
        .len();

    let mut out = format!("# Diff: {title}");
    let _ = write!(
        out,
        "\nconfidence: structural syntax\ncaveat: diff-to-evidence navigation only; not risk, runtime, or security proof\nfiles: changed={} shown={}\nhunks: total={} shown={}\nsymbols: total={} shown={}",
        total_files,
        files.len(),
        total_hunks,
        shown_hunks,
        total_symbols,
        shown_symbols
    );

    if files.is_empty() {
        out.push_str("\n\nNo diff evidence in selected scope.");
        append_footer(&mut out, offset, limit, total_files, files.len());
        return out;
    }

    for file in files {
        let _ = write!(
            out,
            "\n\n## {}\nstatus: {}",
            file.path,
            file.status.as_str()
        );
        if let Some(old_path) = file.old_path.as_ref().filter(|old| *old != &file.path) {
            let _ = write!(out, "\nold-path: {old_path}");
        }
        out.push_str("\nhunks:");
        for hunk in &file.hunks {
            let range = display_hunk_range(hunk);
            if let Some(symbol) = &hunk.symbol {
                let _ = write!(out, "\n- {range} inside {}", symbol.name);
            } else {
                let _ = write!(out, "\n- {range} file-level");
            }
        }
        append_changed_symbols(&mut out, file);
        append_next_reads(&mut out, file);
    }

    append_footer(&mut out, offset, limit, total_files, files.len());
    out
}

fn append_changed_symbols(out: &mut String, file: &DiffFile) {
    let mut seen = HashSet::new();
    let mut symbols = Vec::new();
    for hunk in &file.hunks {
        if let Some(symbol) = &hunk.symbol {
            let key = (symbol.name.clone(), symbol.start_line, symbol.end_line);
            if seen.insert(key) {
                symbols.push((symbol, hunk));
            }
        }
    }
    if symbols.is_empty() {
        return;
    }
    out.push_str("\n\nchanged symbols:");
    for (symbol, hunk) in symbols {
        let _ = write!(
            out,
            "\n- {} :{}-{} modified lines {}",
            symbol.name,
            symbol.start_line,
            symbol.end_line,
            display_changed_line_span(hunk)
        );
    }
}

fn append_next_reads(out: &mut String, file: &DiffFile) {
    let mut actions = Vec::new();
    if let Some(first_hunk) = file.hunks.first() {
        let range = if first_hunk.new_lines == 0 {
            display_line_span(first_hunk.old_start, first_hunk.old_lines)
        } else {
            display_line_span(first_hunk.new_start, first_hunk.new_lines)
        };
        actions.push(NextAction::from_evidence(
            format!("srcwalk show {}:{range} -C 20", file.path),
            "read first changed hunk source",
            10,
            EvidenceSource::Text,
            Anchor::file(Path::new(&file.path)),
        ));
    }
    if let Some(symbol) = file
        .hunks
        .iter()
        .filter_map(|hunk| hunk.symbol.as_ref())
        .find(|symbol| symbol.kind == OutlineKind::Function)
    {
        actions.push(NextAction::from_evidence(
            format!("srcwalk review {}:{}", file.path, symbol.name),
            "review changed function target",
            20,
            EvidenceSource::Ast,
            Anchor::lines(Path::new(&file.path), symbol.start_line, symbol.end_line),
        ));
    }
    actions.push(NextAction::metadata(
        format!("srcwalk deps {}", file.path),
        "inspect changed file dependencies",
        30,
    ));

    let rendered = render_next_actions(&actions);
    if !rendered.is_empty() {
        let _ = write!(out, "\n\n{rendered}");
    }
}

fn append_footer(
    out: &mut String,
    offset: usize,
    limit: Option<usize>,
    total: usize,
    shown: usize,
) {
    let mut pagination_next = String::new();
    if let Some(limit) = limit {
        let next_offset = offset.saturating_add(shown);
        if next_offset < total {
            let omitted = total - next_offset;
            let _ = write!(out, "\n\n## omitted\n- files: {omitted}");
            let rendered = render_next_actions(&[NextAction::metadata(
                format!(
                    "{omitted} more changed files: add --offset {next_offset} --limit {limit}."
                ),
                "diff pagination",
                10,
            )]);
            if !rendered.is_empty() {
                pagination_next = rendered;
            }
        }
    }

    let tokens = estimate_tokens(out.len() as u64);
    let _ = write!(out, "\n\n(~{tokens} tokens)");
    if !pagination_next.is_empty() {
        let _ = write!(out, "\n{pagination_next}");
    }
}

pub(crate) fn display_changed_line_span(hunk: &DiffHunk) -> String {
    if hunk.new_lines == 0 {
        format!("old:{}", display_line_span(hunk.old_start, hunk.old_lines))
    } else {
        display_line_span(hunk.new_start, hunk.new_lines)
    }
}

pub(crate) fn display_hunk_range(hunk: &DiffHunk) -> String {
    if hunk.new_lines == 0 {
        format!("old:{}", display_line_span(hunk.old_start, hunk.old_lines))
    } else {
        format!(":{}", display_line_span(hunk.new_start, hunk.new_lines))
    }
}

pub(crate) fn display_line_span(start: u32, lines: u32) -> String {
    if lines <= 1 {
        start.to_string()
    } else {
        format!("{}-{}", start, hunk_end(start, lines))
    }
}

fn hunk_end(start: u32, lines: u32) -> u32 {
    start.saturating_add(lines.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::{count_lines_cheap, parse_diff_git_paths};

    #[test]
    fn quoted_diff_paths_allow_b_space_inside_filename() {
        let parsed = parse_diff_git_paths(r#""a/src/has b/ marker.rs" "b/src/has b/ marker.rs""#)
            .expect("quoted diff paths should parse");
        assert_eq!(parsed.0.as_deref(), Some("src/has b/ marker.rs"));
        assert_eq!(parsed.1, "src/has b/ marker.rs");
    }

    #[test]
    fn cheap_line_count_counts_lines_without_string_load() {
        let path = std::env::temp_dir().join(format!(
            "srcwalk_diff_line_count_{}_{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
        assert_eq!(count_lines_cheap(&path).unwrap(), 3);
        let _ = std::fs::remove_file(path);
    }
}
