use std::process;

pub(crate) fn run_version(check: bool) {
    let current = env!("CARGO_PKG_VERSION");
    println!("srcwalk {current}");

    if !check {
        return;
    }

    match fetch_latest_version() {
        Ok(latest) => {
            println!("latest {latest}");
            if version_is_newer(&latest, current) {
                println!();
                println!("Update available:");
                println!("  npm install -g srcwalk@latest");
                println!();
                println!("Other install methods:");
                println!("  cargo install srcwalk --locked --force");
                if let Some(target) = release_target() {
                    println!(
                        "  curl -L https://github.com/sting8k/srcwalk/releases/latest/download/srcwalk-{target}.tar.gz | tar xz -C ~/.local/bin"
                    );
                }
            } else {
                println!("Already up to date.");
            }
        }
        Err(err) => {
            eprintln!("error: could not check latest srcwalk release.");
            eprintln!();
            eprintln!("Tried:");
            for source in err.split(';') {
                let label = source
                    .split_once(':')
                    .map_or(source, |(label, _)| label)
                    .trim();
                if !label.is_empty() {
                    eprintln!("  - {label}");
                }
            }
            eprintln!();
            eprintln!("Update manually:");
            eprintln!("  npm install -g srcwalk@latest");
            eprintln!("  cargo install srcwalk --locked --force");
            process::exit(1);
        }
    }
}

type VersionFetchAttempt = (&'static str, fn() -> Result<String, String>);

fn fetch_latest_version() -> Result<String, String> {
    let attempts: &[VersionFetchAttempt] = &[
        ("GitHub latest via curl", fetch_github_latest_with_curl),
        ("GitHub latest via wget", fetch_github_latest_with_wget),
        ("npm registry via curl", fetch_npm_latest_with_curl),
        ("npm registry via wget", fetch_npm_latest_with_wget),
        ("npm CLI", fetch_npm_latest_with_npm),
    ];

    let mut errors = Vec::new();
    for (label, attempt) in attempts {
        match attempt() {
            Ok(version) => return Ok(version),
            Err(err) => errors.push(format!("{label}: {err}")),
        }
    }

    Err(errors.join("; "))
}

fn fetch_github_latest_with_curl() -> Result<String, String> {
    let output = process::Command::new("curl")
        .args([
            "-fsSI",
            "--max-time",
            "5",
            "https://github.com/sting8k/srcwalk/releases/latest",
        ])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    command_stdout(output, "curl").and_then(|headers| {
        parse_latest_tag_from_headers(&headers)
            .ok_or_else(|| "missing latest release redirect".to_string())
    })
}

fn fetch_github_latest_with_wget() -> Result<String, String> {
    let output = process::Command::new("wget")
        .args([
            "--server-response",
            "--spider",
            "--max-redirect=0",
            "--timeout=5",
            "https://github.com/sting8k/srcwalk/releases/latest",
        ])
        .output()
        .map_err(|e| format!("could not run wget: {e}"))?;

    // `wget --spider` writes headers to stderr and may exit non-zero on a 302
    // when redirects are disabled. Treat parseable headers as success.
    let mut headers = String::new();
    headers.push_str(&String::from_utf8_lossy(&output.stdout));
    headers.push_str(&String::from_utf8_lossy(&output.stderr));
    parse_latest_tag_from_headers(&headers)
        .ok_or_else(|| format!("wget exited with {}", output.status))
}

fn fetch_npm_latest_with_curl() -> Result<String, String> {
    let output = process::Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "5",
            "https://registry.npmjs.org/srcwalk/latest",
        ])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    command_stdout(output, "curl").and_then(|json| {
        parse_npm_version(&json).ok_or_else(|| "missing npm version field".to_string())
    })
}

fn fetch_npm_latest_with_wget() -> Result<String, String> {
    let output = process::Command::new("wget")
        .args([
            "-qO-",
            "--timeout=5",
            "https://registry.npmjs.org/srcwalk/latest",
        ])
        .output()
        .map_err(|e| format!("could not run wget: {e}"))?;
    command_stdout(output, "wget").and_then(|json| {
        parse_npm_version(&json).ok_or_else(|| "missing npm version field".to_string())
    })
}

fn fetch_npm_latest_with_npm() -> Result<String, String> {
    let output = process::Command::new("npm")
        .args(["view", "srcwalk", "version", "--silent"])
        .output()
        .map_err(|e| format!("could not run npm: {e}"))?;
    command_stdout(output, "npm").map(|s| s.trim().to_string())
}

fn command_stdout(output: process::Output, command: &str) -> Result<String, String> {
    if !output.status.success() {
        return Err(format!("{command} exited with {}", output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("invalid UTF-8 response: {e}"))
}

fn parse_latest_tag_from_headers(headers: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("location") {
            return None;
        }
        let tag = value.trim().rsplit('/').next()?;
        Some(tag.trim_start_matches('v').to_string())
    })
}

fn parse_npm_version(json: &str) -> Option<String> {
    parse_json_string_field(json, "version")
}

fn parse_json_string_field(json: &str, field: &str) -> Option<String> {
    let marker = format!("\"{field}\"");
    let start = json.find(&marker)?;
    let after_marker = &json[start + marker.len()..];
    let colon = after_marker.find(':')?;
    let after_colon = after_marker[colon + 1..].trim_start();
    let quoted = after_colon.strip_prefix('"')?;
    let end = quoted.find('"')?;
    Some(quoted[..end].to_string())
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    parse_semver(latest) > parse_semver(current)
}

fn parse_semver(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split(['.', '-']);
    let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
    (major, minor, patch)
}

fn release_target() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "main_version_tests.rs"]
mod version_tests;
