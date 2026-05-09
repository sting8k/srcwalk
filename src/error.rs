use std::path::PathBuf;

/// Every error srcwalk can produce. Displayed as user-facing messages with suggestions.
#[derive(Debug)]
pub enum SrcwalkError {
    NotFound {
        path: PathBuf,
        suggestion: Option<String>,
    },
    NoMatches {
        query: String,
        scope: PathBuf,
        suggestion: Option<String>,
    },
    PathLikeNotFound {
        path: PathBuf,
        scope: PathBuf,
        basename: Option<String>,
    },
    PermissionDenied {
        path: PathBuf,
    },
    InvalidQuery {
        query: String,
        reason: String,
    },
    IoError {
        path: PathBuf,
        source: std::io::Error,
    },
    ParseError {
        path: PathBuf,
        reason: String,
    },
    WithNote {
        note: String,
        source: Box<SrcwalkError>,
    },
}

impl std::fmt::Display for SrcwalkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { path, suggestion } => {
                write!(f, "not found: {}", path.display())?;
                if let Some(s) = suggestion {
                    write!(f, " — did you mean: {s}")?;
                }
                Ok(())
            }
            Self::NoMatches {
                query,
                scope,
                suggestion,
            } => {
                write!(f, "no matches for \"{query}\" in {}", scope.display())?;
                if let Some(s) = suggestion {
                    write!(f, "\n> Did you mean: {s}")?;
                }
                Ok(())
            }
            Self::PathLikeNotFound {
                path,
                scope,
                basename,
            } => {
                writeln!(f, "not found: {}", path.display())?;
                writeln!(
                    f,
                    "> Caveat: this looks like a file path, but no file exists at that path under {}.",
                    scope.display()
                )?;
                let _ = basename;
                write!(f, "> Next: check the path or scope.")
            }
            Self::PermissionDenied { path } => {
                write!(f, "{} [permission denied]", path.display())
            }
            Self::InvalidQuery { query, reason } => {
                write!(f, "invalid query \"{query}\": {reason}")
            }
            Self::IoError { path, source } => {
                write!(f, "{}: {source}", path.display())
            }
            Self::ParseError { path, reason } => {
                write!(f, "parse error in {}: {reason}", path.display())
            }
            Self::WithNote { note, source } => write!(f, "{note}\n\n{source}"),
        }
    }
}

impl std::error::Error for SrcwalkError {}

impl SrcwalkError {
    /// Exit code matching the spec.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NotFound { .. }
            | Self::NoMatches { .. }
            | Self::PathLikeNotFound { .. }
            | Self::IoError { .. } => 2,
            Self::InvalidQuery { .. } | Self::ParseError { .. } => 3,
            Self::PermissionDenied { .. } => 4,
            Self::WithNote { source, .. } => source.exit_code(),
        }
    }
    pub(crate) fn unsupported_syntax(query: &str, action: &str, supported: &[String]) -> Self {
        Self::InvalidQuery {
            query: query.to_string(),
            reason: format!(
                "unsupported syntax for `{action}`. Supported:\n  {}",
                supported.join("\n  ")
            ),
        }
    }
}
