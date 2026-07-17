//! Single-port server: same TCP port serves the HTML page and the WebSocket.
//! Discriminates by the `Upgrade: websocket` header (SPEC §2.1).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use base64::Engine;
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

use crate::monitors::LayoutHandle;
use crate::mouse::MouseCmd;
use crate::processor::Processor;
use crate::protocol::ClientMsg;

/// Embedded HTML client — the whole UI in one file (SPEC §13.1).
const CLIENT_HTML: &str = include_str!("../client/index.html");

/// Favicon (48x48 PNG), derived from `src/icon.png` at build time (see build.rs).
const FAVICON_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/favicon.png"));

/// Apple touch icon (180x180 PNG) for iOS "Add to Home Screen".
const APPLE_TOUCH_PNG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/apple-touch-icon.png"));

const MAX_HEAD: usize = 16 * 1024;

#[derive(Clone)]
pub struct Ctx {
    pub layout: LayoutHandle,
    pub mouse_tx: Sender<MouseCmd>,
    pub debug: bool,
    /// True while at least one phone is connected (read by the tray).
    pub connected: Arc<AtomicBool>,
}

/// Bind and serve forever. `tls` is `None` when running in plain-HTTP fallback.
pub async fn run(port: u16, tls: Option<Arc<rustls::ServerConfig>>, ctx: Ctx) -> Result<()> {
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    let acceptor = tls.map(TlsAcceptor::from);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("accept error: {e}");
                continue;
            }
        };
        let ctx = ctx.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let res = match acceptor {
                Some(acc) => match acc.accept(stream).await {
                    Ok(tls_stream) => handle(tls_stream, ctx).await,
                    Err(e) => {
                        tracing::debug!("tls handshake from {peer} failed: {e}");
                        return;
                    }
                },
                None => handle(stream, ctx).await,
            };
            if let Err(e) = res {
                tracing::debug!("connection from {peer} ended: {e}");
            }
        });
    }
}

/// Read the HTTP request head, then branch into WebSocket or page delivery.
async fn handle<S>(mut stream: S, ctx: Ctx) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let head = read_head(&mut stream).await?;
    let text = String::from_utf8_lossy(&head);

    if let Some(key) = websocket_key(&text) {
        // WebSocket handshake: complete it by hand on the already-(TLS-)opened
        // stream, then hand off to tungstenite in server role.
        let accept = derive_accept_key(key.as_bytes());
        let resp = format!(
            "HTTP/1.1 101 Switching Protocols\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        stream.write_all(resp.as_bytes()).await?;
        stream.flush().await?;

        let ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        serve_ws(ws, ctx).await;
        Ok(())
    } else {
        match request_path(&text) {
            Some(p) if p.starts_with("/favicon") => {
                serve_bytes(&mut stream, "image/png", "max-age=86400", FAVICON_PNG).await
            }
            Some(p) if p.starts_with("/apple-touch-icon") => {
                serve_bytes(&mut stream, "image/png", "max-age=86400", APPLE_TOUCH_PNG).await
            }
            Some(p) if p.starts_with("/mousee-ca.cer") => {
                let ca = crate::tls::ca_der()?;
                serve_bytes(&mut stream, "application/x-x509-ca-cert", "no-store", &ca).await
            }
            _ => {
                serve_bytes(
                    &mut stream,
                    "text/html; charset=utf-8",
                    "no-store",
                    client_page().as_bytes(),
                )
                .await
            }
        }
    }
}

/// Extract the request-target (path) from the HTTP request line, e.g. `/favicon.ico`.
fn request_path(req: &str) -> Option<&str> {
    req.lines().next()?.split_whitespace().nth(1)
}

/// Read until the end of HTTP headers (`\r\n\r\n`), bounded by `MAX_HEAD`.
async fn read_head<S>(stream: &mut S) -> Result<Vec<u8>>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break; // EOF
        }
        buf.extend_from_slice(&chunk[..n]);
        if find_crlf2(&buf).is_some() {
            break;
        }
        if buf.len() > MAX_HEAD {
            anyhow::bail!("request head too large");
        }
    }
    Ok(buf)
}

fn find_crlf2(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Return the value of `Sec-WebSocket-Key` if this is a WebSocket upgrade.
fn websocket_key(req: &str) -> Option<String> {
    let mut upgrade_ws = false;
    let mut key = None;
    for line in req.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "upgrade" if value.eq_ignore_ascii_case("websocket") => upgrade_ws = true,
            "sec-websocket-key" => key = Some(value.to_string()),
            _ => {}
        }
    }
    if upgrade_ws {
        key
    } else {
        None
    }
}

/// The client page with the apple-touch-icon inlined as a `data:` URI.
///
/// iOS fetches `apple-touch-icon` in a *separate* request when adding the page
/// to the Home Screen, and that request rejects our self-signed certificate —
/// so a normal `href="/apple-touch-icon.png"` silently fails and iOS falls back
/// to a letter glyph. Inlining the PNG avoids the extra fetch entirely.
fn client_page() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(APPLE_TOUCH_PNG);
        let data_uri = format!("data:image/png;base64,{b64}");
        CLIENT_HTML.replace("__APPLE_TOUCH_ICON__", &data_uri)
    })
}

/// Write a single `200 OK` response with the given content type, cache policy
/// and body (the page and both icons all go through here).
async fn serve_bytes<S>(stream: &mut S, content_type: &str, cache: &str, body: &[u8]) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: {cache}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

async fn serve_ws<S>(mut ws: WebSocketStream<S>, ctx: Ctx)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tracing::info!("phone connected");
    ctx.connected.store(true, Ordering::Relaxed);
    let mut proc = Processor::new(ctx.layout.clone(), ctx.debug);

    while let Some(msg) = ws.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!("ws error: {e}");
                break;
            }
        };
        if msg.is_close() {
            break;
        }
        if !msg.is_text() {
            continue;
        }
        let text = match msg.to_text() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let parsed: ClientMsg = match serde_json::from_str(text) {
            Ok(p) => p,
            Err(e) => {
                tracing::debug!("bad message: {e} ({text})");
                continue;
            }
        };
        for cmd in proc.handle(parsed) {
            // Drop on a closed channel rather than crash (soft degradation).
            let _ = ctx.mouse_tx.send(cmd);
        }
    }
    ctx.connected.store(false, Ordering::Relaxed);
    tracing::info!("phone disconnected");
}
