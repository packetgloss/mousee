#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! mousee — control your PC mouse from phone orientation sensors.
//! One Rust binary (HTML client embedded), one TCP port for page + WebSocket.
//!
//! Release workers on Windows use the GUI subsystem, so `--background` never
//! creates a console. Interactive release launches create or attach a console
//! for the QR and prompts. Debug builds always stay in one foreground process
//! so logs and Ctrl-C behave normally.

#[cfg(all(windows, feature = "tray"))]
mod autostart;
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
#[command(name = "mousee", version, about = "Phone orientation -> PC mouse")]
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

    /// Verbose sensor/mapping logs; also stay in the foreground without a tray.
    #[arg(long)]
    debug: bool,

    /// Internal: run as the detached background worker (server + tray).
    #[arg(long, hide = true)]
    background: bool,
}

fn init_logging(debug: bool) {
    let level = if debug { "mousee=debug" } else { "mousee=info" };
    let mut filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    if debug {
        // A machine-wide `RUST_LOG=warn` must not make a development launch
        // silently lose the coordinate diagnostics it promises.
        filter = filter.add_directive("mousee=debug".parse().expect("valid log directive"));
    }
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}

/// Only interactive release launches use the detached worker + tray model.
/// Development and explicit diagnostic/headless modes remain foreground-only.
#[cfg(all(windows, feature = "tray"))]
fn use_background_worker(args: &Args) -> bool {
    !cfg!(debug_assertions) && !args.debug && !args.no_tray && !args.yes
}

/// Build the runtime, start the server, and return the runtime + connection flag.
fn start_server(args: &Args, ip: Ipv4Addr) -> Result<(tokio::runtime::Runtime, Arc<AtomicBool>)> {
    let layout = monitors::LayoutHandle::detect();
    layout.current().log_summary();
    layout.spawn_watcher(); // pick up monitor hotplug at runtime (SPEC §6)

    let mouse_tx = mouse::spawn();

    let tls_cfg = if args.no_tls {
        None
    } else {
        match tls::server_config(ip) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                tracing::warn!("could not enable TLS ({e}); falling back to HTTP");
                None
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
        debug: args.debug || cfg!(debug_assertions),
        connected: connected.clone(),
    };
    let port = args.port;
    rt.spawn(async move {
        if let Err(e) = server::run(port, tls_cfg, ctx).await {
            tracing::error!("server error: {e}");
        }
    });

    Ok((rt, connected))
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
    let preferred = args
        .ip
        .or_else(|| config::PREFERRED_IP.and_then(|s| s.parse().ok()));
    let ip = preferred
        .or_else(|| net::candidates().first().map(|candidate| candidate.ip))
        .unwrap_or(Ipv4Addr::LOCALHOST);
    let (_rt, _connected) = start_server(&args, ip)?;

    #[cfg(feature = "tray")]
    {
        tray::run(ip, args.port, _connected); // takes over the thread (diverges)
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
    let (rt, _connected) = start_server(&args, ip)?;
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
    // No `--debug`: the detached worker has no console and installs no log
    // subscriber, so logging is foreground-only by design (run with --no-tray
    // to see logs). Forwarding --debug here would just be a silent no-op.
    cmd.creation_flags(DETACHED_PROCESS);
    cmd.spawn().context("spawning background worker")?;
    Ok(())
}

fn main() -> Result<()> {
    // A release binary is a GUI-subsystem executable to keep the autostart
    // worker truly windowless. Attach to the invoking terminal when there is
    // one; otherwise create the interactive console before Clap prints output.
    #[cfg(all(windows, not(debug_assertions)))]
    if !std::env::args_os().any(|arg| arg == "--background") {
        instance::ensure_console();
    }

    let args = Args::parse();

    // Detached background worker has no console, so it installs no log
    // subscriber: logging is foreground-only by design (use --no-tray to see it).
    if args.background && !cfg!(debug_assertions) {
        instance::hide_console();
        return run_worker(args);
    }

    // If a background worker is already running in the tray, don't spin up a
    // second one. Do this *before* any console output so we can hide the window
    // and show only the native notice.
    #[cfg(all(windows, feature = "tray"))]
    if use_background_worker(&args) && instance::is_running() {
        instance::warn_already_running();
        return Ok(());
    }

    // `cargo run` is a development tool: debug builds are always verbose and
    // stay in this process. A release build can opt into the same behavior with
    // --debug, which also prevents a detached/tray worker below.
    init_logging(args.debug || cfg!(debug_assertions));
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Pick the LAN-IP, then make sure a cert exists so the QR can show the right
    // scheme. Generating it here warms the on-disk cache that the server process
    // (background worker or foreground) reuses, so TLS is actually set up once.
    let preferred = args
        .ip
        .or_else(|| config::PREFERRED_IP.and_then(|s| s.parse().ok()));
    let ip = tui::choose_ip(preferred, args.yes)?;
    let scheme = if args.no_tls {
        tracing::warn!("TLS disabled (--no-tls): iPhone will not grant sensor access");
        "http"
    } else {
        match tls::ensure_cert(ip) {
            Ok(_) => "https",
            Err(e) => {
                tracing::warn!("could not enable TLS ({e}); falling back to HTTP");
                "http"
            }
        }
    };
    let url = format!("{scheme}://{ip}:{}", args.port);

    #[cfg(all(windows, feature = "tray"))]
    if use_background_worker(&args) {
        if let Err(e) = autostart::prompt() {
            tracing::warn!("could not configure Windows autostart: {e}");
        }
    }

    println!("\n  mousee — page + WebSocket on 0.0.0.0:{}", args.port);
    tui::print_qr(&url);

    // Default (Windows, tray build): run the app in a detached background worker
    // so this console is disposable — close it with the X and the tray lives on.
    #[cfg(all(windows, feature = "tray"))]
    {
        if use_background_worker(&args) {
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
