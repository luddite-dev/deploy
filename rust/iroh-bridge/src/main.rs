use std::{net::SocketAddr, time::Duration};

use anyhow::{anyhow, Result};
use tokio::net::TcpListener;

use iroh_bridge::{http::router, network::Network, state::AppState};

const DEFAULT_ADDR: &str = "127.0.0.1:7777";

const HELP: &str = "\
luddite iroh-bridge: Iroh transport sidecar for the luddite control plane.

USAGE:
    iroh-bridge [--addr <ADDR>] [--help]

OPTIONS:
        --addr <ADDR>    Local address to bind the sidecar HTTP API on.
                         Defaults to the env variable LUDDITE_SIDECAR_ADDR
                         or 127.0.0.1:7777 if unset.
    -h, --help           Print this help and exit.
";

enum Parsed {
    Run { addr: Option<String> },
    Help,
}

fn parse_args(args: &[String]) -> Result<Parsed> {
    let mut addr: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--addr" => {
                i += 1;
                let v = args
                    .get(i)
                    .ok_or_else(|| anyhow!("--addr requires a value"))?;
                addr = Some(v.clone());
            }
            s if s.starts_with("--addr=") => {
                addr = Some(s["--addr=".len()..].to_string());
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
        i += 1;
    }
    Ok(Parsed::Run { addr })
}

fn resolve_addr(flag: Option<String>, env: Option<String>) -> String {
    flag.or(env)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ADDR.to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = std::env::var("LUDDITE_SIDECAR_ADDR").ok();
    match parse_args(&args)? {
        Parsed::Help => {
            print!("{HELP}");
            Ok(())
        }
        Parsed::Run { addr: flag } => {
            let raw_addr = resolve_addr(flag, env);
            let bind_addr: SocketAddr = raw_addr.parse()?;
            let state = AppState::new(String::new());
            let network = Network::bind(state.clone()).await?;
            network.refresh_identity().await?;
            eprintln!("iroh-bridge: sidecar http on {bind_addr}");

            tokio::spawn({
                let network = network.clone();
                async move {
                    loop {
                        if let Err(e) = network.flush_outbound_once().await {
                            eprintln!("flush_outbound: {e}");
                        }
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                }
            });

            let listener = TcpListener::bind(bind_addr).await?;
            axum::serve(listener, router(state)).await?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_no_flag_no_env_uses_default() {
        assert_eq!(resolve_addr(None, None), DEFAULT_ADDR);
    }

    #[test]
    fn resolve_env_when_no_flag() {
        assert_eq!(
            resolve_addr(None, Some("127.0.0.1:9999".into())),
            "127.0.0.1:9999"
        );
    }

    #[test]
    fn resolve_flag_overrides_env() {
        assert_eq!(
            resolve_addr(
                Some("127.0.0.1:8888".into()),
                Some("127.0.0.1:9999".into())
            ),
            "127.0.0.1:8888"
        );
    }

    #[test]
    fn resolve_empty_env_uses_default() {
        assert_eq!(resolve_addr(None, Some(String::new())), DEFAULT_ADDR);
    }

    #[test]
    fn parse_help_short_and_long() {
        assert!(matches!(parse_args(&args(&["--help"])).unwrap(), Parsed::Help));
        assert!(matches!(parse_args(&args(&["-h"])).unwrap(), Parsed::Help));
    }

    #[test]
    fn parse_addr_space_separated() {
        match parse_args(&args(&["--addr", "127.0.0.1:7000"])).unwrap() {
            Parsed::Run { addr } => assert_eq!(addr.as_deref(), Some("127.0.0.1:7000")),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn parse_addr_equals_form() {
        match parse_args(&args(&["--addr=127.0.0.1:7000"])).unwrap() {
            Parsed::Run { addr } => assert_eq!(addr.as_deref(), Some("127.0.0.1:7000")),
            _ => panic!("expected Run"),
        }
    }

    #[test]
    fn parse_addr_without_value_errors() {
        assert!(parse_args(&args(&["--addr"])).is_err());
    }

    #[test]
    fn parse_unknown_arg_errors() {
        assert!(parse_args(&args(&["--bogus"])).is_err());
    }
}
