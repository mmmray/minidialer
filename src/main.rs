use std::io;

use anyhow::Error;
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::{filter::LevelFilter, EnvFilter};

mod browser;
mod command;
mod curl;

#[derive(Parser, Debug)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: CliSubcommand,
}

#[derive(Subcommand, Debug)]
enum CliSubcommand {
    Browser(BrowserCli),
    Command(CommandCli),
    CurlWs(CurlWsCli),
    CurlTcp(CurlTcpCli),
}

#[derive(Args, Debug)]
struct BrowserCli {
    /// which upstream websocket URL to connect to. start with wss:// or ws://
    upstream: String,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug)]
struct CommandCli {
    #[command(flatten)]
    common: CliCommon,

    command: Vec<String>,
}

#[derive(Args, Debug)]
struct CurlWsCli {
    /// which upstream websocket URL to connect to. start with wss:// or ws://
    upstream: String,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug)]
struct CurlTcpCli {
    /// which upstream websocket URL to connect to, for example:
    ///
    /// example.com
    /// example.com:80
    /// [::1]:443
    /// 127.0.0.1:443
    ///
    /// default ports are 80 and 443 depending on the value of the `tls` flag.
    upstream: String,

    /// Turn off TLS, and instead forward TCP connections as-is.
    ///
    /// Without TLS, this proxy is basically useless in terms of fingerprinting resistance, and
    /// behaves like a TCP port forwarder, but turning it off is still useful for internal testing.
    #[arg(long)]
    no_tls: bool,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug)]
struct CliCommon {
    /// which local host to listen to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// which local port to listen to
    #[arg(long, default_value_t = 3000)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // write logs to stderr so stdout can be locked from subcommands
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .with_writer(io::stderr)
        .init();

    let args = Cli::parse();

    match args.command {
        CliSubcommand::Browser(args) => {
            browser::main(args).await;
        }
        CliSubcommand::Command(args) => {
            command::main(args).await;
        }
        CliSubcommand::CurlWs(args) => {
            curl::ws::main(args).await?;
        }
        CliSubcommand::CurlTcp(args) => {
            curl::tcp::main(args).await;
        }
    }

    Ok(())
}
