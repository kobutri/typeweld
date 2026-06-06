use std::{
    io::{self, Write},
    net::SocketAddr,
};

use clap::{Parser, Subcommand};
use typeweld_cli::contract_to_json;

#[derive(Debug, Parser)]
#[command(name = "e2e-axum-effect-server", about = "Run the e2e Axum server")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the API contract as JSON.
    Contract,
    /// Serve the API over HTTP.
    Serve {
        #[arg(default_value = "127.0.0.1:3000")]
        addr: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match Cli::parse().command.unwrap_or_else(|| Command::Serve {
        addr: default_addr(),
    }) {
        Command::Contract => println!("{}", contract_to_json(&e2e_axum_effect_server::contract())?),
        Command::Serve { addr } => serve(addr).await?,
    }

    Ok(())
}

fn default_addr() -> SocketAddr {
    "127.0.0.1:3000"
        .parse()
        .expect("default listen address is valid")
}

async fn serve(addr: SocketAddr) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;

    println!("LISTENING http://{local_addr}");
    io::stdout().flush()?;

    axum::serve(listener, e2e_axum_effect_server::app()).await?;
    Ok(())
}
