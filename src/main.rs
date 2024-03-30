use clap::{Args, Parser, Subcommand};

mod browser;
mod command;

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
struct CliCommon {
    /// which local host to listen to
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// which local port to listen to
    #[arg(long, default_value_t = 3000)]
    port: u16,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Cli::parse();

    match args.command {
        CliSubcommand::Browser(args) => {
            browser::main(args).await;
        }
        CliSubcommand::Command(args) => {
            command::main(args).await;
        }
    }
}
