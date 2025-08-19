use anyhow::Result;
use pacm::cli::PacmCli;

fn main() {
    if let Err(e) = real_main() {
        eprintln!("pacm error: {:#}", e);
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = PacmCli::parse();
    cli.run()
}
