//! mousee — control your PC mouse from a phone via its gyroscope.
//! One Rust binary (HTML client embedded), one TCP port for page + WebSocket.

mod config;
mod monitors;
mod mouse;
mod net;
mod processor;
mod protocol;
mod server;
mod tls;
mod tui;

use std::net::Ipv4Addr;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "mousee", version, about = "Phone gyroscope -> PC mouse")]
struct Args {
    /// Force a specific LAN-IP (skips autodetection & interface picker).
    #[arg(long)]
    ip: Option<Ipv4Addr>,

    /// TCP port for the page + WebSocket.
    #[arg(long, default_value_t = config::DEFAULT_PORT)]
    port: u16,

    /// Skip the interactive interface picker (use recommended/forced IP).
    #[arg(long, visible_alias = "no-tui")]
    yes: bool,

    /// Disable TLS and serve plain HTTP (iPhone will NOT grant sensors).
    #[arg(long)]
    no_tls: bool,

    /// Verbose, throttled per-frame mapping logs.
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let level = if args.debug { "mousee=debug" } else { "mousee=info" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)))
        .with_target(false)
        .without_time()
        .init();

    // Install the ring crypto provider for rustls (pure Rust, no system OpenSSL).
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Read the monitor layout once at startup (SPEC §6).
    let layout = Arc::new(monitors::Layout::detect());
    layout.log_summary();

    // Dedicated mouse thread.
    let mouse_tx = mouse::spawn();

    // Choose the advertised LAN-IP.
    let preferred = args
        .ip
        .or_else(|| config::PREFERRED_IP.and_then(|s| s.parse().ok()));
    let ip = tui::choose_ip(preferred, args.yes)?;

    // Build TLS (or fall back to plain HTTP with a warning — SPEC §2.2).
    let (tls_cfg, scheme) = if args.no_tls {
        tracing::warn!("TLS disabled (--no-tls): iPhone will not grant sensor access");
        (None, "http")
    } else {
        match tls::server_config(ip) {
            Ok(cfg) => (Some(cfg), "https"),
            Err(e) => {
                tracing::warn!("could not enable TLS ({e}); falling back to HTTP. iPhone will not grant sensors.");
                (None, "http")
            }
        }
    };

    let url = format!("{scheme}://{ip}:{}", args.port);

    println!("\n  mousee server starting");
    println!("  serving page + WebSocket on 0.0.0.0:{}", args.port);
    tui::print_qr(&url);
    println!("  Press Ctrl-C to quit.\n");

    let ctx = server::Ctx {
        layout,
        mouse_tx,
        debug: args.debug,
    };

    let server = tokio::spawn(server::run(args.port, tls_cfg, ctx));

    tokio::select! {
        r = server => {
            if let Ok(Err(e)) = r {
                tracing::error!("server error: {e}");
            }
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\n  shutting down.");
        }
    }

    Ok(())
}
