mod bans;
mod config;
mod conn;
mod console;
mod limits;
mod state;
mod tls;
mod users;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::bans::BanList;
use crate::config::ServerConfig;
use crate::state::Server;
use crate::users::Registry;

const USAGE: &str = "\
Chumly server

Usage: chumly-server [--config <path to server.toml>]

By default the config is read from server.toml in the current directory.
If the file does not exist, it is created with default settings.
The certificate (cert.der, key.der), bans (bans.toml) and registered
users (users.toml) are stored next to the config.";

/// Logs a newer release if GitHub has one. In the background: the server
/// does not wait for GitHub, and a failure is only a debug line.
fn check_for_update() {
    let spawned =
        std::thread::Builder::new().name("update-check".into()).spawn(|| match shared::release::fetch_latest() {
            Ok(r) if shared::release::is_newer(&r.version, shared::VERSION) => {
                warn!("a newer version is available: {} (you have {}): {}", r.version, shared::VERSION, r.page);
            }
            Ok(r) => info!("up to date (latest release {})", r.version),
            Err(e) => tracing::debug!("update check failed: {e:#}"),
        });
    if let Err(e) = spawned {
        tracing::debug!("update check not started: {e}");
    }
}

fn parse_args() -> anyhow::Result<PathBuf> {
    let mut args = std::env::args().skip(1);
    let mut config = PathBuf::from("server.toml");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => config = args.next().context("--config requires a path")?.into(),
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown argument '{other}'\n\n{USAGE}"),
        }
    }
    Ok(config)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let config_path = parse_args()?;
    let dir = match config_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => Path::new(".").to_path_buf(),
    };

    let (cfg, created) = ServerConfig::load_or_create(&config_path)?;
    if created {
        info!("created config {}", config_path.display());
    }
    let bans = BanList::load(&dir.join("bans.toml")).context("reading bans.toml")?;
    let registry = Registry::load(&dir.join("users.toml")).context("reading users.toml")?;
    let (cert, key, new_key) = tls::load_or_create(&dir)?;
    if new_key && !registry.users.is_empty() {
        warn!(
            "a new server key was created but users.toml has {} registered users: users' keys are derived from the server's key, so they will all come with new keys. Bind them back with the console command: rebind <registered nick> <online nick>",
            registry.users.len()
        );
    }
    let fingerprint = shared::key_fingerprint(&cert).context("cert.der is not a valid certificate")?;

    let bind: SocketAddr = cfg.bind.parse().with_context(|| format!("invalid bind address '{}'", cfg.bind))?;
    let endpoint = quinn::Endpoint::server(tls::server_config(cert, key, cfg.public_status)?, bind)
        .with_context(|| format!("failed to open UDP port {bind}"))?;

    info!("server '{}' listening on {bind} (UDP)", cfg.name);
    info!("channels: {}", cfg.channels.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", "));
    info!("server password: {}", if cfg.password.is_empty() { "none" } else { "set" });
    info!("server key fingerprint: {fingerprint}");
    info!("type help for a list of commands");
    if cfg.check_updates {
        check_for_update();
    }

    let server = Arc::new(Server::new(cfg, bans, registry));
    if let Some(code) = server.lock().setup_code.clone() {
        warn!("no owner yet. Admin setup code: {code}");
        warn!("enter it in the client (Become owner) to become the server owner; it works once");
    }
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::unbounded_channel();
    console::spawn(server.clone(), shutdown_tx);

    loop {
        tokio::select! {
            incoming = endpoint.accept() => match incoming {
                Some(i) => { tokio::spawn(conn::handle(server.clone(), i)); }
                None => break,
            },
            // None means the process has no stdin (service, nohup):
            // the console is just unavailable, this is not a stop command.
            Some(()) = shutdown_rx.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    info!("shutting down server");
    server.lock().notice("Server is shutting down.");
    tokio::time::sleep(Duration::from_millis(300)).await;
    endpoint.close(0u32.into(), b"shutdown");
    let _ = tokio::time::timeout(Duration::from_secs(2), endpoint.wait_idle()).await;
    Ok(())
}
