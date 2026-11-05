use clap::error::ErrorKind;
use clap::Parser;
use zen_cli::Cli;

#[tokio::main]
async fn main() {
    // Exit-code migration Q1 (docs/exit-codes.md): clap usage errors
    // exit VALIDATION (4), not clap's default 2 -- 2 is AUTH in the
    // B2 table. Help and version keep their conventional exit 0.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            let code = match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => 0,
                _ => zen_cli::exit_code::VALIDATION,
            };
            let _ = err.print();
            std::process::exit(code);
        }
    };
    match zen_cli::run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // Q5: a CodedError keeps the table code it picked at the
            // source; the blanket GENERAL stays only as the fallback.
            let code = zen_cli::exit_code_for_error(&err);
            eprintln!("Error: {}", err);
            std::process::exit(code);
        }
    }
}
