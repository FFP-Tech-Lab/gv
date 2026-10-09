#![forbid(unsafe_code)]

mod commands;
mod download;
mod error;
#[cfg(feature = "tui")]
mod pick;
mod platform;
mod resolve;
mod shim;
mod store;
mod ui;

#[cfg(test)]
mod testutil;

use std::env;
use std::future::Future;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::error::Error;
use crate::ui::Tone;

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
        /// Versions to install. Omit them in a terminal to choose one
        #[arg(value_name = "VERSION")]
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

struct RunError {
    error: Error,
    quiet: bool,
    shim: bool,
}

impl From<Error> for RunError {
    fn from(error: Error) -> Self {
        Self {
            error,
            quiet: false,
            shim: false,
        }
    }
}

impl From<std::io::Error> for RunError {
    fn from(error: std::io::Error) -> Self {
        Self::from(Error::from(error))
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            report_error(&err);
            ExitCode::from(1)
        }
    }
}

fn report_error(err: &RunError) {
    let ui = ui::Ui::detect(err.quiet);
    for line in err.error.to_string().lines() {
        if line.is_empty() {
            continue;
        }
        if err.shim {
            eprintln!("gv: {line}");
        } else if ui.animated() {
            eprint!("{}", ui::format_result(Tone::Failed, line, ui.color()));
        } else {
            eprintln!("gv: {line}");
        }
    }
}

fn run() -> Result<(), RunError> {
    if let Some(tool) = shim::invoked_tool() {
        return shim::execute(tool).map_err(|error| RunError {
            error,
            quiet: false,
            shim: true,
        });
    }

    let cli = Cli::parse();
    let root = store::gv_root()?;
    store::ensure_layout(&root)?;
    shim::install_shims(&root)?;

    match cli.command {
        Command::Install { versions, quiet } => {
            let Some(versions) =
                resolve_install_versions(&root, versions, quiet).map_err(|error| RunError {
                    error,
                    quiet,
                    shim: false,
                })?
            else {
                return Ok(());
            };
            block_on(commands::install::run(&root, &versions, quiet)).map_err(|error| {
                RunError {
                    error,
                    quiet,
                    shim: false,
                }
            })?;
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
            let version = match resolve_use_version(&root, &cwd, version, unset)? {
                UseRequest::Ready(version) => version,
                #[cfg(feature = "tui")]
                UseRequest::Cancel => return Ok(()),
            };
            let outcome = commands::use_version::run(&root, &cwd, version.as_deref(), global)?;
            let tone = if outcome.notice {
                Tone::Notice
            } else {
                Tone::Done
            };
            ui::Ui::detect(false).finish(tone, &outcome.message);
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
            commands::clean::run(&root)?;
        }
        Command::Shell { version, unset } => {
            let version = commands::take_version(version, unset)?;
            print!("{}", commands::shell::script(version.as_deref())?);
        }
    }
    Ok(())
}

fn resolve_install_versions(
    root: &Path,
    versions: Vec<String>,
    quiet: bool,
) -> Result<Option<Vec<String>>, Error> {
    if !versions.is_empty() {
        return Ok(Some(versions));
    }
    pick_install_version(root, quiet)
}

#[cfg(feature = "tui")]
fn pick_install_version(root: &Path, quiet: bool) -> Result<Option<Vec<String>>, Error> {
    if !ui::Ui::detect(quiet).animated() {
        return Err(Error::InstallVersionRequired);
    }
    match block_on(pick::choose_remote(root))? {
        Some(version) => Ok(Some(vec![version])),
        None => Ok(None),
    }
}

#[cfg(not(feature = "tui"))]
fn pick_install_version(_root: &Path, _quiet: bool) -> Result<Option<Vec<String>>, Error> {
    Err(Error::InstallVersionRequired)
}

enum UseRequest {
    #[cfg(feature = "tui")]
    Cancel,
    Ready(Option<String>),
}

fn resolve_use_version(
    root: &Path,
    cwd: &Path,
    version: Option<String>,
    unset: bool,
) -> Result<UseRequest, Error> {
    if version.is_none() && !unset {
        if let Some(request) = pick_use_version(root, cwd)? {
            return Ok(request);
        }
    }
    Ok(UseRequest::Ready(commands::take_version(version, unset)?))
}

#[cfg(feature = "tui")]
fn pick_use_version(root: &Path, cwd: &Path) -> Result<Option<UseRequest>, Error> {
    if !ui::Ui::detect(false).animated() {
        return Ok(None);
    }
    let gv_version = env::var("GV_VERSION").ok();
    let request = match pick::choose_installed(root, cwd, gv_version.as_deref())? {
        Some(version) => UseRequest::Ready(Some(version)),
        None => UseRequest::Cancel,
    };
    Ok(Some(request))
}

#[cfg(not(feature = "tui"))]
fn pick_use_version(_root: &Path, _cwd: &Path) -> Result<Option<UseRequest>, Error> {
    Ok(None)
}

fn block_on<T>(future: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(future)
}
