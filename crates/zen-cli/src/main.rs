use clap::Parser;
use clap::error::ErrorKind;
use zen_cli::Cli;

/// Stack for the thread that runs the real main. Windows gives the
/// main thread a 1 MB stack, and `Cli::try_parse` alone already needs
/// ~920 KB of it in debug builds: the clap tree carries 16 top-level
/// subcommands, several with nested subcommands. DS-7's
/// register/deregister pushed the full binary to ~1010 KB, so on
/// Windows the process aborted on startup with no output at all and
/// windows-check failed every test that spawns the binary. Running
/// the real main on a thread with an explicit 8 MB stack keeps the
/// CLI usable no matter how many subcommands the tree gains.
const MAIN_STACK_SIZE: usize = 8 * 1024 * 1024;

fn main() {
    let child = std::thread::Builder::new()
        .stack_size(MAIN_STACK_SIZE)
        .spawn(real_main)
        .expect("spawn main thread");
    if child.join().is_err() {
        std::process::exit(1);
    }
}

#[tokio::main]
async fn real_main() {
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
