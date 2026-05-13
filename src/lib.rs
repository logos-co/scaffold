pub(crate) type DynResult<T> = anyhow::Result<T>;

pub(crate) mod cli;
pub(crate) mod cli_help;
pub(crate) mod commands;
pub(crate) mod config;
pub(crate) mod constants;
pub(crate) mod doctor_checks;
pub(crate) mod error;
pub(crate) mod migrate;
pub(crate) mod model;
pub(crate) mod process;
pub(crate) mod project;
pub(crate) mod repo;
pub(crate) mod state;
pub(crate) mod template;

pub fn run(args: Vec<String>) -> DynResult<()> {
    cli::run(args)
}

pub fn entry_main() {
    let args: Vec<String> = std::env::args().collect();
    if let Err(err) = run(args) {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}
