use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::colors::*;

mod util;
mod install;
mod remove;

pub use install::cmd_install;
pub use remove::cmd_remove;

#[derive(Parser, Debug)]
#[command(
    name = "pacm",
    version,
    about = "Fast, cache-first JavaScript/TypeScript package manager",
    long_about = "pacm — a blazing fast, cache-first package manager.\n\nExamples:\n  pacm init --name my-app\n  pacm install\n  pacm add axios\n  pacm cache path\n  pacm cache clean"
)]
pub struct PacmCli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Init { #[arg(long)] name: Option<String>, #[arg(long)] version: Option<String> },
    Remove { packages: Vec<String> },
    Install { packages: Vec<String>, #[arg(long, short='D')] dev: bool, #[arg(long)] optional: bool, #[arg(long="no-save")] no_save: bool, #[arg(long)] exact: bool, #[arg(long)] prefer_offline: bool, #[arg(long)] no_progress: bool },
    Add { package: String, #[arg(long, short='D')] dev: bool, #[arg(long)] optional: bool, #[arg(long="no-save")] no_save: bool, #[arg(long)] exact: bool },
    List,
    Cache { #[command(subcommand)] cmd: CacheCmd },
    Pm { #[command(subcommand)] cmd: PmCmd },
}

#[derive(Subcommand, Debug)]
pub enum CacheCmd { Path, Clean }

#[derive(Subcommand, Debug)]
pub enum PmCmd {
    Lockfile { #[arg(long, short='f', default_value="json")] format: String, #[arg(long, short='s')] save: bool },
    Prune,
    Ls,
}

impl PacmCli {
    pub fn parse() -> Self { <Self as Parser>::parse() }

    pub fn run(&self) -> Result<()> {
        match &self.command {
            None => { self.print_help(); Ok(()) }
            Some(Commands::Init { name, version }) => super::cmd_init(name.clone(), version.clone()),
            Some(Commands::Install { packages, dev, optional, no_save, exact, prefer_offline, no_progress }) => install::cmd_install(packages.clone(), *dev, *optional, *no_save, *exact, *prefer_offline, *no_progress),
            Some(Commands::Add { package, dev, optional, no_save, exact }) => install::cmd_install(vec![package.clone()], *dev, *optional, *no_save, *exact, false, false),
            Some(Commands::Remove { packages }) => remove::cmd_remove(packages.clone()),
            Some(Commands::List) => super::cmd_list(),
            Some(Commands::Cache { cmd }) => match cmd { CacheCmd::Path => super::cmd_cache_path(), CacheCmd::Clean => super::cmd_cache_clean() },
            Some(Commands::Pm { cmd }) => match cmd { PmCmd::Lockfile { format, save } => super::cmd_pm_lockfile(format.clone(), *save), PmCmd::Prune => super::cmd_pm_prune(), PmCmd::Ls => super::cmd_list() },
        }
    }

    fn print_help(&self) {
        println!("pacm - Fast, cache-first package manager\n");
        println!("Commands:\n  init [--name --version]\n  install [pkg..] [--dev|--optional] [--no-save] [--prefer-offline] [--no-progress]\n  add <pkg> [--dev|--optional] [--no-save]\n  remove <pkg..>\n  list\n  cache <path|clean>\n  pm <lockfile|prune|ls> [options]");
    }
}
