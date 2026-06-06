use std::{
    env,
    io::{self, Write},
    net::SocketAddr,
    process::ExitCode,
};

use api_collector::contract_to_json;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "serve".to_owned());

    match command.as_str() {
        "contract" => {
            let json = contract_to_json(&e2e_axum_effect_server::contract())
                .map_err(|error| format!("failed to serialize contract: {error}"))?;
            println!("{json}");
            Ok(())
        }
        "serve" => {
            let addr = args
                .next()
                .unwrap_or_else(|| "127.0.0.1:3000".to_owned())
                .parse::<SocketAddr>()
                .map_err(|error| format!("invalid listen address: {error}"))?;
            serve(addr).await
        }
        "--help" | "-h" | "help" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown e2e server command `{other}`")),
    }
}

async fn serve(addr: SocketAddr) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| format!("failed to bind {addr}: {error}"))?;
    let local_addr = listener
        .local_addr()
        .map_err(|error| format!("failed to read local address: {error}"))?;

    println!("LISTENING http://{local_addr}");
    io::stdout()
        .flush()
        .map_err(|error| format!("failed to flush listen address: {error}"))?;

    axum::serve(listener, e2e_axum_effect_server::app())
        .await
        .map_err(|error| format!("server failed: {error}"))
}

fn print_usage() {
    println!("usage:\n  e2e-axum-effect-server contract\n  e2e-axum-effect-server serve [addr]");
}
