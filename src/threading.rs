//! Shared thread-count policy for CLI pools and filesystem walkers.

pub const MAX_THREADS: usize = 24;

pub fn configured_rayon_threads() -> Result<usize, String> {
    configured_threads(default_rayon_threads())
}

pub fn configured_walker_threads() -> Result<usize, String> {
    configured_threads(default_walker_threads())
}

fn configured_threads(default: usize) -> Result<usize, String> {
    match std::env::var("SRCWALK_THREADS") {
        Ok(raw) => parse_thread_count(&raw),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err("SRCWALK_THREADS must be valid UTF-8 and an integer in 1..=24".to_string())
        }
    }
}

fn parse_thread_count(raw: &str) -> Result<usize, String> {
    let parsed = raw.parse::<usize>().map_err(|_| {
        format!("SRCWALK_THREADS must be an integer in 1..={MAX_THREADS}, got {raw:?}")
    })?;

    if (1..=MAX_THREADS).contains(&parsed) {
        Ok(parsed)
    } else {
        Err(format!(
            "SRCWALK_THREADS must be in 1..={MAX_THREADS}, got {parsed}"
        ))
    }
}

fn default_rayon_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| (n.get() / 2).clamp(2, 6))
}

fn default_walker_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| {
        let logical = n.get();
        if logical <= 8 {
            logical
        } else {
            (logical * 3 / 4).min(MAX_THREADS)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_thread_count_accepts_bounds() {
        assert_eq!(parse_thread_count("1").unwrap(), 1);
        assert_eq!(parse_thread_count("24").unwrap(), 24);
    }

    #[test]
    fn parse_thread_count_rejects_zero_and_huge_values() {
        assert!(parse_thread_count("0").unwrap_err().contains("1..=24"));
        assert!(parse_thread_count("50000").unwrap_err().contains("1..=24"));
    }

    #[test]
    fn parse_thread_count_rejects_non_numbers() {
        assert!(parse_thread_count("many").unwrap_err().contains("integer"));
    }
}
