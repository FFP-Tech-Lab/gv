#![forbid(unsafe_code)]

mod commands;
mod download;
mod error;
mod platform;
mod resolve;
mod shim;
mod store;
mod ui;

#[cfg(test)]
mod testutil;

use std::env;
use std::future::Future;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::error::Error;

#[derive(Parser)]
#[command(
    name = "gv",
    version,
    about = "Go version manager",
    subcommand_required = true,
    arg_required_else_help = true,
    after_long_help = "Examples:\n  gv install 1.23.4\n  gv install latest\n  gv install 1.22.5 1.23.4\n  gv uninstall 1.22.5 1.23.4\n  gv use 1.23.4\n  gv use --unset\n  gv use --global 1.23.4\n  gv use --global --unset\n  gv list-remote 1.23\n  gv list-remote --offline\n  gv which\n  gv clean\n  eval \"$(gv init bash)\"\n  eval \"$(gv completions bash)\"\n  eval \"$(gv shell 1.23.4)\""
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download and install one or more Go versions
    Install {
        /// Skip progress bars and print one line of transferred bytes when each version finishes
        #[arg(short, long)]
        quiet: bool,
        /// Version numbers; more than one may be passed
        #[arg(required = true, value_name = "VERSION")]
        versions: Vec<String>,
    },
    /// Uninstall one or more Go versions
    Uninstall {
        /// Uninstall even when the global version points at a target, and clear the global version file
        #[arg(long)]
        force: bool,
        /// Version numbers; more than one may be passed
        #[arg(required = true, value_name = "VERSION")]
        versions: Vec<String>,
    },
    /// List installed versions
    List {
        /// Output format: text or json
        #[arg(long, value_enum, default_value = "text")]
        output: commands::list::OutputFormat,
    },
    /// List remote versions that can be installed
    #[command(name = "list-remote")]
    ListRemote {
        /// Include non-stable versions such as beta and rc
        #[arg(long)]
        all: bool,
        /// Ignore the local cache and download the index again
        #[arg(long, conflicts_with = "offline")]
        refresh: bool,
        /// Use the cached index and do not download
        #[arg(long, conflicts_with = "refresh")]
        offline: bool,
        /// Output format: text or json
        #[arg(long, value_enum, default_value = "text")]
        output: commands::list::OutputFormat,
        /// Show versions for this minor, exact version, or prefix
        #[arg(value_name = "PREFIX")]
        prefix: Option<String>,
    },
    /// Select a Go version
    Use {
        /// Version number. Errors when combined with --unset
        version: Option<String>,
        /// Write the global version instead of .go-version in the current directory
        #[arg(long)]
        global: bool,
        /// Remove .go-version in the current directory, or the global version file with --global
        #[arg(long)]
        unset: bool,
    },
    /// Show the active version and where it came from
    Current,
    /// Print the current version, its source, and the paths of go and gofmt
    Which,
    /// Print the bash or zsh init snippet
    Init { shell: String },
    /// Print the bash or zsh completion script
    Completions { shell: String },
    /// Delete cached archives without removing installed versions
    Clean,
    /// Print an export GV_VERSION statement, or a statement that unsets it
    Shell {
        /// Version number. Errors when combined with --unset
        version: Option<String>,
        /// Print a statement that unsets GV_VERSION
        #[arg(long)]
        unset: bool,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            for line in err.to_string().lines() {
                eprintln!("gv: {line}");
            }
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Error> {
    if let Some(tool) = shim::invoked_tool() {
        return shim::execute(tool);
    }

    let cli = Cli::parse();
    let root = store::gv_root()?;
    store::ensure_layout(&root)?;
    shim::install_shims(&root)?;

    match cli.command {
        Command::Install { versions, quiet } => {
            block_on(commands::install::run(&root, &versions, quiet))?
        }
        Command::Uninstall { versions, force } => {
            commands::uninstall::run(&root, &versions, force)?;
        }
        Command::List { output } => {
            let cwd = env::current_dir()?;
            let gv_version = env::var("GV_VERSION").ok();
            let report =
                commands::list::installed_report(&root, &cwd, gv_version.as_deref(), output)?;
            if let Some(warning) = report.warning {
                eprintln!("gv: {warning}");
            }
            if commands::list::version_tui_enabled(output) {
                commands::list::browse("Installed versions", &report.rows);
            }
            print!("{}", report.stdout);
        }
        Command::ListRemote {
            all,
            refresh,
            offline,
            output,
            prefix,
        } => {
            block_on(commands::list::remote(
                &root,
                all,
                refresh,
                offline,
                prefix.as_deref(),
                output,
            ))?;
        }
        Command::Use {
            version,
            global,
            unset,
        } => {
            let cwd = env::current_dir()?;
            let version = commands::take_version(version, unset)?;
            let outcome = commands::use_version::run(&root, &cwd, version.as_deref(), global)?;
            println!("{}", outcome.message);
            if let Some(hint) = outcome.hint {
                eprintln!("gv: {hint}");
            }
        }
        Command::Current => {
            let cwd = env::current_dir()?;
            let gv_version = env::var("GV_VERSION").ok();
            print!(
                "{}",
                commands::current::current(&root, &cwd, gv_version.as_deref())?
            );
        }
        Command::Which => {
            let cwd = env::current_dir()?;
            let gv_version = env::var("GV_VERSION").ok();
            print!(
                "{}",
                commands::which::which(&root, &cwd, gv_version.as_deref())?
            );
        }
        Command::Init { shell } => {
            print!("{}", commands::init::script(&shell)?);
        }
        Command::Completions { shell } => {
            print!("{}", commands::completions::script(&shell)?);
        }
        Command::Clean => {
            print!("{}", commands::clean::run(&root)?);
        }
        Command::Shell { version, unset } => {
            let version = commands::take_version(version, unset)?;
            print!("{}", commands::shell::script(version.as_deref())?);
        }
    }
    Ok(())
}

fn block_on<T>(future: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}
