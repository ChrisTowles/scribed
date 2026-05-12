fn main() -> std::process::ExitCode {
    match scribed::cli::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("scribed: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
