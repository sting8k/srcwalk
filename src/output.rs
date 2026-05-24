use srcwalk::error::SrcwalkError;

pub(crate) fn emit_result(result: Result<String, SrcwalkError>) {
    match result {
        Ok(s) => emit_output(&s),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(e.exit_code());
        }
    }
}

pub(crate) fn emit_output(output: &str) {
    println!("{output}");
}
