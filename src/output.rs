use std::io::{self, Write};
use std::process;

pub(crate) fn emit_result(
    result: Result<String, srcwalk::error::SrcwalkError>,
    query: &str,
    json: bool,
) {
    match result {
        Ok(output) => {
            if json {
                let json = serde_json::json!({
                    "query": query,
                    "output": output,
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json)
                        .expect("serde_json::Value is always serializable")
                );
            } else {
                emit_output(&output);
            }
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(e.exit_code());
        }
    }
}

/// Write output directly to stdout for deterministic agent/script capture.
pub(crate) fn emit_output(output: &str) {
    print!("{output}");
    let _ = io::stdout().flush();
}
