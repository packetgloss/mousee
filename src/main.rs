//! mousee — control your PC mouse from a phone via its gyroscope.
//! One Rust binary (HTML client embedded), one TCP port for page + WebSocket.
//!
//! Process model on Windows: the interactive launcher (this console) picks the
//! interface, prints the QR, then spawns a **detached, console-less background
//! worker** (`--background`) that runs the server + tray. Closing the launcher
//! console therefore does not kill the running app — it lives in the tray until
//! you choose Quit there.

mod config;
mod instance;
mod monitors;
mod mouse;
mod net;
mod processor;
mod protocol;
mod server;
mod tls;
#[cfg(feature = "tray")]
mod tray;
mod tui;

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicBool;
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

    /// Stay in the foreground (console + logs, Ctrl-C to quit); no tray.
    #[arg(long)]
    no_tray: bool,

    /// Verbose, throttled per-frame mapping logs.
    #[arg(long)]
    debug: bool,

    /// Internal: run as the detached background worker (server + tray).
    #[arg(long, hide = true)]
    background: bool,

    /// Internal: print the QR for --url in this (fresh) console window.
    #[arg(long, hide = true)]
    show_qr: bool,

    /// Internal: URL used by --show-qr.
    #[arg(long, hide = true)]
    url: Option<String>,
}

/// Hidden `--show-qr` mode: a standalone console window that prints the QR and
/// the URL, then waits so the user can scan it. Spawned by the tray.
fn run_show_qr(url: String) -> Result<()> {
    tui::print_qr(&url);
    println!("  Press Enter or close this window when done.");
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    Ok(())
}

fn init_logging(debug: bool) {
    let level = if debug { "mousee=debug" } else { "mousee=info" };
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)))
        .with_target(false)
        .without_time()
        .init();
}

/// Build the runtime, start the server, return the runtime + connection flag +
/// the URL scheme that was actually achieved (TLS may have fallen back to HTTP).
fn start_server(
    args: &Args,
    ip: Ipv4Addr,
) -> Result<(tokio::runtime::Runtime, Arc<AtomicBool>, &'static str)> {
    let layout = monitors::LayoutHandle::detect();
    layout.current().log_summary();
    layout.spawn_watcher(); // pick up monitor hotplug at runtime (SPEC §6)

    let mouse_tx = mouse::spawn();

    let (tls_cfg, scheme) = if args.no_tls {
        (None, "http")
    } else {
        match tls::server_config(ip) {
            Ok(cfg) => (Some(cfg), "https"),
            Err(e) => {
                tracing::warn!("could not enable TLS ({e}); falling back to HTTP");
                (None, "http")
            }
        }
    };

    let connected = Arc::new(AtomicBool::new(false));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let ctx = server::Ctx {
        layout,
        mouse_tx,
        debug: args.debug,
        connected: connected.clone(),
    };
    let port = args.port;
    rt.spawn(async move {
        if let Err(e) = server::run(port, tls_cfg, ctx).await {
            tracing::error!("server error: {e}");
        }
    });

    Ok((rt, connected, scheme))
}

/// The detached background worker: server + tray, no console, silent logs.
fn run_worker(args: Args) -> Result<()> {
    // Own the single-instance mutex for our whole lifetime; if another worker
    // already holds it, exit immediately. (Held until the process ends — when
    // the tray feature is on, tray::run never returns, so this stays alive.)
    let _instance = match instance::acquire() {
        Some(g) => g,
        None => return Ok(()),
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let ip = args.ip.unwrap_or(Ipv4Addr::LOCALHOST);
    let (_rt, _connected, _scheme) = start_server(&args, ip)?;

    #[cfg(feature = "tray")]
    {
        let url = format!("{_scheme}://{ip}:{}", args.port);
        tray::run(url, ip, args.port, _connected); // takes over the thread (diverges)
    }

    #[cfg(not(feature = "tray"))]
    {
        _rt.block_on(async {
            let _ = tokio::signal::ctrl_c().await;
        });
        Ok(())
    }
}

/// Foreground mode: keep the server in this console and wait for Ctrl-C.
fn run_foreground(args: Args, ip: Ipv4Addr) -> Result<()> {
    let (rt, _connected, _scheme) = start_server(&args, ip)?;
    println!("  Running in the foreground. Press Ctrl-C to quit.\n");
    rt.block_on(async {
        let _ = tokio::signal::ctrl_c().await;
    });
    println!("\n  shutting down.");
    Ok(())
}

/// Spawn the background worker as a detached, console-less process (Windows).
#[cfg(all(windows, feature = "tray"))]
fn spawn_worker(args: &Args, ip: Ipv4Addr) -> Result<()> {
    use anyhow::Context;
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--background")
        .arg("--ip")
        .arg(ip.to_string())
        .arg("--port")
        .arg(args.port.to_string());
    if args.no_tls {
        cmd.arg("--no-tls");
    }
    if args.debug {
        cmd.arg("--debug");
    }
    cmd.creation_flags(DETACHED_PROCESS);
    cmd.spawn().context("spawning background worker")?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Standalone QR console window spawned by the tray.
    if args.show_qr {
        return run_show_qr(args.url.unwrap_or_default());
    }

    // Detached background worker has no console: don't init stdout logging.
    if args.background {
        return run_worker(args);
    }

    // If a background worker is already running in the tray, don't spin up a
    // second one. Do this *before* any console output so we can hide the window
    // and show only the native notice.
    #[cfg(all(windows, feature = "tray"))]
    if !args.no_tray && instance::is_running() {
        instance::warn_already_running();
        return Ok(());
    }

    init_logging(args.debug);
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Pick the LAN-IP and determine the scheme (this also caches the cert that
    // the background worker will reuse).
    let preferred = args
        .ip
        .or_else(|| config::PREFERRED_IP.and_then(|s| s.parse().ok()));
    let ip = tui::choose_ip(preferred, args.yes)?;
    let scheme = if args.no_tls {
        tracing::warn!("TLS disabled (--no-tls): iPhone will not grant sensor access");
        "http"
    } else {
        match tls::server_config(ip) {
            Ok(_) => "https",
            Err(e) => {
                tracing::warn!("could not enable TLS ({e}); falling back to HTTP");
                "http"
            }
        }
    };
    let url = format!("{scheme}://{ip}:{}", args.port);

    println!("\n  mousee — page + WebSocket on 0.0.0.0:{}", args.port);
    tui::print_qr(&url);

    // Default (Windows, tray build): run the app in a detached background worker
    // so this console is disposable — close it with the X and the tray lives on.
    #[cfg(all(windows, feature = "tray"))]
    {
        if !args.no_tray && !args.yes {
            spawn_worker(&args, ip)?;
            println!("  mousee is now running in the background — see the tray icon.");
            println!("  You can CLOSE THIS WINDOW; the app keeps running in the tray.");
            println!("  Quit any time from the tray icon → Quit.\n");
            // Keep the console open so the QR stays visible until the user closes it.
            loop {
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        }
    }

    // Otherwise (foreground requested, non-Windows, or no tray feature).
    run_foreground(args, ip)
}
