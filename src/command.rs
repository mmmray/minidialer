use std::process::Stdio;

use tokio::net::TcpStream;
use tokio::process::Command;

use crate::CommandCli;

pub async fn main(args: CommandCli) {
    let addr = format!("{}:{}", args.common.host, args.common.port);
    tracing::info!(
        "listening on {}, forwarding to command: {:?}",
        addr,
        args.command
    );

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::spawn(process_connection(socket, args.command.clone()));
    }
}

async fn process_connection(mut socket: TcpStream, commandline: Vec<String>) {
    tracing::debug!("spawning command");
    let mut command = match Command::new(&commandline[0])
        .args(&commandline[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!("failed to spawn command: {:?}", e);
            return;
        }
    };

    let stdin = command.stdin.take().unwrap();
    let stdout = command.stdout.take().unwrap();

    let mut stdio_combined = tokio::io::join(stdout, stdin);

    let _ = tokio::io::copy_bidirectional(&mut socket, &mut stdio_combined).await;
    let _ = command.kill();
    tracing::debug!("stopping command");
}
