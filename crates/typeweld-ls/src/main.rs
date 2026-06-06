use std::process::ExitCode;

fn main() -> ExitCode {
    match typeweld_ls::run_stdio() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
