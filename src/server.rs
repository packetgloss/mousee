//! Single-port server: same TCP port serves the HTML page and the WebSocket.
//! Discriminates by the `Upgrade: websocket` header (SPEC §2.1).

use std::net::SocketAddr;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::WebSocketStream;

use crate::monitors::Layout;
use crate::mouse::MouseCmd;
use crate::processor::Processor;
use crate::protocol::ClientMsg;

/// Embedded HTML client — the whole UI in one file (SPEC §13.1).
const CLIENT_HTML: &str = include_str!("../client/index.html");

const MAX_HEAD: usize = 16 * 1024;

#[derive(Clone)]
pub struct Ctx {
    pub layout: Arc<Layout>,
    pub mouse_tx: Sender<MouseCmd>,
    pub debug: bool,
}

/// Bind and serve forever. `tls` is `None` when running in plain-HTTP fallback.
pub async fn run(
    port: u16,
    tls: Option<Arc<rustls::ServerConfig>>,
    ctx: Ctx,
) -> Result<()> {
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
        serve_page(&mut stream).await
    }
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

async fn serve_page<S>(stream: &mut S) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let body = CLIENT_HTML.as_bytes();
    let resp = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-store\r\n\
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
    tracing::info!("phone disconnected");
}
