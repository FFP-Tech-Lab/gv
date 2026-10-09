#![forbid(unsafe_code)]

mod commands;
mod download;
mod error;
mod platform;
mod resolve;
mod shim;
mod store;

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
    about = "Go 版本管理器",
    subcommand_required = true,
    arg_required_else_help = true,
    after_long_help = "示例：\n  gv install 1.23.4\n  gv install latest\n  gv install 1.22.5 1.23.4\n  gv uninstall 1.22.5 1.23.4\n  gv use 1.23.4\n  gv use --unset\n  gv use --global 1.23.4\n  gv use --global --unset\n  gv which\n  gv clean\n  eval \"$(gv init bash)\"\n  eval \"$(gv completions bash)\"\n  eval \"$(gv shell 1.23.4)\""
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 下载并安装一个或多个 Go 版本
    Install {
        /// 不绘制进度条，每个版本结束时只打印一行已传输字节数
        #[arg(short, long)]
        quiet: bool,
        /// 版本号，可一次传入多个
        #[arg(required = true, value_name = "VERSION")]
        versions: Vec<String>,
    },
    /// 卸载一个或多个 Go 版本
    Uninstall {
        /// 全局版本指向待卸载版本时仍卸载，并清除全局版本文件
        #[arg(long)]
        force: bool,
        /// 版本号，可一次传入多个
        #[arg(required = true, value_name = "VERSION")]
        versions: Vec<String>,
    },
    /// 列出已安装的版本
    List {
        /// 输出格式：text 或 json
        #[arg(long, value_enum, default_value = "text")]
        output: commands::list::OutputFormat,
    },
    /// 列出可安装的远端版本
    #[command(name = "list-remote")]
    ListRemote {
        /// 包含 beta、rc 等非 stable 版本
        #[arg(long)]
        all: bool,
        /// 忽略本地缓存，重新下载索引
        #[arg(long)]
        refresh: bool,
        /// 输出格式：text 或 json
        #[arg(long, value_enum, default_value = "text")]
        output: commands::list::OutputFormat,
    },
    /// 选择 Go 版本
    Use {
        /// 版本号。与 --unset 一起使用时会报错
        version: Option<String>,
        /// 写入全局版本，而不是当前目录的 .go-version
        #[arg(long)]
        global: bool,
        /// 删除当前目录的 .go-version，或与 --global 一起删除全局版本文件
        #[arg(long)]
        unset: bool,
    },
    /// 显示当前生效的版本和来源
    Current,
    /// 打印当前版本、来源，以及 go 与 gofmt 的路径
    Which,
    /// 打印 bash 或 zsh 的初始化片段
    Init { shell: String },
    /// 打印 bash 或 zsh 的补全脚本
    Completions { shell: String },
    /// 删除安装包缓存，不删除已安装的版本
    Clean,
    /// 打印 export GV_VERSION 或取消该变量的语句
    Shell {
        /// 版本号。与 --unset 一起使用时会报错
        version: Option<String>,
        /// 打印取消 GV_VERSION 的语句
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
            print!("{}", report.stdout);
        }
        Command::ListRemote {
            all,
            refresh,
            output,
        } => {
            block_on(commands::list::remote(&root, all, refresh, output))?;
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
