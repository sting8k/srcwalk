use std::path::Path;

use crate::cache::OutlineCache;
use crate::error::SrcwalkError;
use crate::format::rel_nonempty;
use crate::types::{Match, SearchResult};

use super::display;

struct GeneralFilter {
    field: String,
    value: String,
}

fn parse_general_filters(filter: Option<&str>) -> Result<Vec<GeneralFilter>, SrcwalkError> {
    let Some(filter) = filter else {
        return Ok(Vec::new());
    };
    let mut filters = Vec::new();
    for part in filter.split_whitespace() {
        let Some((field, value)) = part.split_once(':') else {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filters must use field:value qualifiers".to_string(),
            });
        };
        let field = field.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        if field.is_empty() || value.is_empty() {
            return Err(SrcwalkError::InvalidQuery {
                query: filter.to_string(),
                reason: "filter field and value cannot be empty".to_string(),
            });
        }
        match field.as_str() {
            "path" | "file" | "text" | "kind" => filters.push(GeneralFilter { field, value }),
            "args" | "receiver" | "recv" | "caller" => {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: format!("filter qualifier `{field}` only applies with --callers"),
                });
            }
            _ => {
                return Err(SrcwalkError::InvalidQuery {
                    query: filter.to_string(),
                    reason: format!(
                        "unsupported filter field `{field}`; use path, file, text, or kind"
                    ),
                });
            }
        }
    }
    Ok(filters)
}

pub fn apply_general_filter(
    result: &mut SearchResult,
    scope: &Path,
    cache: &OutlineCache,
    filter: Option<&str>,
) -> Result<(), SrcwalkError> {
    let filters = parse_general_filters(filter)?;
    if filters.is_empty() {
        return Ok(());
    }
    result
        .matches
        .retain(|m| filters.iter().all(|f| f.matches(m, scope, cache)));
    result.total_found = result.matches.len();
    result.definitions = result.matches.iter().filter(|m| m.is_definition).count();
    result.comments = result.matches.iter().filter(|m| m.in_comment).count();
    result.usages = result.matches.len().saturating_sub(result.definitions);
    result.has_more = false;
    result.offset = 0;
    Ok(())
}

impl GeneralFilter {
    fn matches(&self, m: &Match, scope: &Path, cache: &OutlineCache) -> bool {
        match self.field.as_str() {
            "path" => rel_nonempty(&m.path, scope).contains(&self.value),
            "file" => m
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(&self.value)),
            "text" => m.text.contains(&self.value),
            "kind" => display::match_kind_label(m, cache).is_some_and(|kind| kind == self.value),
            _ => false,
        }
    }
}
