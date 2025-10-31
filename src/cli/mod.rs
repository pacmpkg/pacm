use anyhow::Result;
use clap::{Parser, Subcommand};

pub mod commands;

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
    /// Create a new package.json
    Init {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        version: Option<String>,
    },
    /// Remove one or more dependencies
    Remove {
        packages: Vec<String>,
    },
    /// Install all dependencies or add specific packages
    #[command(alias = "i")]
    Install {
        packages: Vec<String>,
        #[arg(long, short = 'D')]
        dev: bool,
        #[arg(long)]
        optional: bool,
        #[arg(long = "no-save")]
        no_save: bool,
        #[arg(long)]
        exact: bool,
        #[arg(long)]
        prefer_offline: bool,
        #[arg(long)]
        no_progress: bool,
        #[arg(long)]
        link: bool,
        #[arg(long)]
        copy: bool,
    },
    /// Alias for install <pkg>
    Add {
        package: String,
        #[arg(long, short = 'D')]
        dev: bool,
        #[arg(long)]
        optional: bool,
        #[arg(long = "no-save")]
        no_save: bool,
        #[arg(long)]
        exact: bool,
        #[arg(long)]
        link: bool,
        #[arg(long)]
        copy: bool,
    },
    List,
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    Pm {
        #[command(subcommand)]
        cmd: PmCmd,
    },
    /// Run lifecycle scripts for packages (preinstall/install/postinstall)
    Scripts {
        #[command(subcommand)]
        cmd: ScriptsCmd,
    },
    /// Run a script from package.json or execute a local binary in node_modules/.bin
    Run {
        /// script name or binary to run; remaining args are passed-through
        #[arg(trailing_var_arg = true, required = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ScriptsCmd {
    /// Run lifecycle scripts for packages
    Run {
        packages: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        ignore_scripts: bool,
        /// Skip confirmation prompts and run immediately
        #[arg(long)]
        yes: bool,
        /// Prompt for each package individually instead of a single confirmation
        #[arg(long = "per-package")]
        per_package: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CacheCmd {
    /// Show the cache path on this machine
    Path,
    /// Clean the cache (remove all cached packages)
    Clean,
}

#[derive(Subcommand, Debug)]
pub enum PmCmd {
    Lockfile {
        #[arg(long, short = 'f', default_value = "json")]
        format: String,
        #[arg(long, short = 's')]
        save: bool,
    },
    Prune,
    Ls,
}

impl PacmCli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    pub fn run(&self) -> Result<()> {
        match &self.command {
            None => {
                self.print_help();
                Ok(())
            }
            Some(Commands::Init { name, version }) => {
                commands::cmd_init(name.clone(), version.clone())
            }
            Some(Commands::Install {
                packages,
                dev,
                optional,
                no_save,
                exact,
                prefer_offline,
                no_progress,
                link,
                copy,
            }) => commands::cmd_install(
                packages.clone(),
                commands::InstallOptions {
                    dev: *dev,
                    optional: *optional,
                    no_save: *no_save,
                    exact: *exact,
                    prefer_offline: *prefer_offline,
                    no_progress: *no_progress,
                    link: *link,
                    copy: *copy,
                },
            ),
            Some(Commands::Add { package, dev, optional, no_save, exact, link, copy }) => {
                commands::cmd_install(
                    vec![package.clone()],
                    commands::InstallOptions {
                        dev: *dev,
                        optional: *optional,
                        no_save: *no_save,
                        exact: *exact,
                        prefer_offline: false,
                        no_progress: false,
                        link: *link,
                        copy: *copy,
                    },
                )
            }
            Some(Commands::Remove { packages }) => commands::cmd_remove(packages.clone()),
            Some(Commands::List) => commands::cmd_list(),
            Some(Commands::Cache { cmd }) => match cmd {
                CacheCmd::Path => commands::cmd_cache_path(),
                CacheCmd::Clean => commands::cmd_cache_clean(),
            },
            Some(Commands::Pm { cmd }) => match cmd {
                PmCmd::Lockfile { format, save } => {
                    commands::cmd_pm_lockfile(format.clone(), *save)
                }
                PmCmd::Prune => commands::cmd_pm_prune(),
                PmCmd::Ls => commands::cmd_list(),
            },
            Some(Commands::Scripts { cmd }) => match cmd {
                ScriptsCmd::Run { packages, all, ignore_scripts, yes, per_package } => {
                    commands::cmd_scripts_run(
                        packages.clone(),
                        *all,
                        *ignore_scripts,
                        *yes,
                        *per_package,
                    )
                }
            },
            Some(Commands::Run { args }) => commands::cmd_run(args.clone()),
        }
    }

    fn print_help(&self) {
        println!("pacm - Fast, cache-first package manager\n");
        println!(
            "Commands:\n  init [--name --version]\n  install [pkg..] [--dev|--optional] [--no-save] [--prefer-offline] [--no-progress]\n  add <pkg> [--dev|--optional] [--no-save]\n  remove <pkg..>\n  list\n  cache <path|clean>\n  pm <lockfile|prune|ls> [options]"
        );
    }
}
