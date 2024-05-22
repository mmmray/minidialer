use std::io;

use anyhow::Error;
use clap::{Args, Parser, Subcommand};
use tracing_subscriber::{filter::LevelFilter, EnvFilter};

mod browser;
mod command;
#[cfg(feature = "curl")]
mod curl;
mod splithttp;
mod tcp_fragment;

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
    #[cfg(feature = "curl")]
    CurlWs(CurlWsCli),
    #[cfg(feature = "curl")]
    CurlTcp(CurlTcpCli),
    TcpFragment(TcpFragmentCli),
    SplitHttp(SplitHttpCli),
    SplitHttpServer(SplitHttpServerCli),
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

#[cfg(feature = "curl")]
#[derive(Args, Debug)]
struct CurlWsCli {
    /// which upstream websocket URL to connect to. start with wss:// or ws://
    upstream: String,

    #[command(flatten)]
    common: CliCommon,
}

#[cfg(feature = "curl")]
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

#[derive(Args, Debug, Clone)]
struct TcpFragmentCli {
    /// for example, example.com:443
    ///
    /// port is mandatory
    upstream: String,

    /// after this string, a new TCP packet will be started.
    ///
    /// only outbound packets are affected. the string may appear multiple times, in which case
    /// multiple packets are affected.
    #[arg(long)]
    split_after: String,

    /// Sleep this many milliseconds between packets. It has been shown that certain middlemen do not
    /// like to keep their reassembly buffers around for longer than 10 seconds.
    ///
    /// Defaults to 10 seconds.
    ///
    /// In the current implementation, setting it to a very low value (< 1) can cause fragmentation
    /// to be disabled, because the fragmentation itself is implemented as just a sleep statement.
    #[arg(long, default_value_t = 5000)]
    split_sleep_ms: u64,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug, Clone)]
struct SplitHttpCli {
    /// for example, https://example.com/subpath/
    upstream: String,

    /// Optionally, a different URL to send the download requests to.
    ///
    /// In the end this URL still needs to (indirectly) point to the same server.
    #[arg(long)]
    download_upstream: Option<String>,

    /// Additional HTTP headers to set (or override)
    #[arg(long, short = 'H')]
    header: Vec<String>,

    /// What is the largest payload that should be uploaded per HTTP request?
    ///
    /// Large values may not pass certain firewalls, small payloads waste a lot of bandwidth to
    /// HTTP overhead.
    #[arg(long, default_value_t = 122880)]
    upload_chunk_size: usize,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug, Clone)]
struct SplitHttpServerCli {
    /// for example, example.com:443
    ///
    /// Port mandatory.
    upstream: String,

    #[command(flatten)]
    common: CliCommon,
}

#[derive(Args, Debug, Clone)]
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
        #[cfg(feature = "curl")]
        CliSubcommand::CurlWs(args) => {
            curl::ws::main(args).await?;
        }
        #[cfg(feature = "curl")]
        CliSubcommand::CurlTcp(args) => {
            curl::tcp::main(args).await;
        }
        CliSubcommand::TcpFragment(args) => {
            tcp_fragment::main(args).await;
        }
        CliSubcommand::SplitHttp(args) => {
            splithttp::client::main(args).await?;
        }
        CliSubcommand::SplitHttpServer(args) => {
            splithttp::server::main(args).await?;
        }
    }

    Ok(())
}
