use std::path::{Path, PathBuf};

use crate::format;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    path: PathBuf,
    range: AnchorRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AnchorRange {
    File,
    Line(u32),
    Lines { start: u32, end: u32 },
}

impl Anchor {
    pub(crate) fn file(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            range: AnchorRange::File,
        }
    }

    pub(crate) fn line(path: &Path, line: u32) -> Self {
        debug_assert!(line > 0);
        Self {
            path: path.to_path_buf(),
            range: AnchorRange::Line(line),
        }
    }

    pub(crate) fn lines(path: &Path, start: u32, end: u32) -> Self {
        debug_assert!(start > 0);
        debug_assert!(end >= start);
        Self {
            path: path.to_path_buf(),
            range: AnchorRange::Lines { start, end },
        }
    }

    pub(crate) const fn start_line(&self) -> u32 {
        match self.range {
            AnchorRange::File => 1,
            AnchorRange::Line(line) | AnchorRange::Lines { start: line, .. } => line,
        }
    }

    pub(crate) fn display(&self) -> String {
        self.display_with_path(&format::display_path(&self.path))
    }

    pub(crate) fn display_relative_to(&self, scope: &Path) -> String {
        self.display_with_path(&format::rel_nonempty(&self.path, scope))
    }

    fn display_with_path(&self, path: &str) -> String {
        match self.range {
            AnchorRange::File => path.to_string(),
            AnchorRange::Line(line) => format!("{path}:{line}"),
            AnchorRange::Lines { start, end } => format!("{path}:{start}-{end}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_anchor_uses_existing_display_path() {
        let path = std::env::current_dir().unwrap().join("src/lib.rs");
        let anchor = Anchor::line(&path, 12);

        assert_eq!(
            anchor.display(),
            format!("{}:12", format::display_path(&path))
        );
    }

    #[test]
    fn range_anchor_uses_existing_relative_display_path() {
        let scope = std::env::current_dir().unwrap().join("src");
        let path = scope.join("lib.rs");
        let anchor = Anchor::lines(&path, 10, 20);

        assert_eq!(
            anchor.display_relative_to(&scope),
            format!("{}:10-20", format::rel_nonempty(&path, &scope))
        );
        assert_eq!(anchor.start_line(), 10);
    }

    #[test]
    fn file_anchor_uses_existing_relative_display_path() {
        let scope = std::env::current_dir().unwrap().join("src");
        let path = scope.join("lib.rs");
        let anchor = Anchor::file(&path);

        assert_eq!(
            anchor.display_relative_to(&scope),
            format::rel_nonempty(&path, &scope)
        );
    }
}
