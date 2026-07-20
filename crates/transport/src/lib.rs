//! Socket adapter: the thin, transport-specific translator between a byte stream
//! and the engine's in-memory API. It owns no protocol logic — it deframes JSON
//! lines into [`Envelope`]s for [`engine::Engine::handle`] and reframes the
//! engine's outbound bus ([`engine::Engine::subscribe`]) back onto the socket.
//!
//! Two modes: [`run_dialer`] (the production reverse tunnel — the device dials
//! out and keeps reconnecting) and [`run_listener`] (a local/USB-OTG listener
//! for development).
//!
//! Framing is JSON-lines: one compact `Envelope` per `\n`-terminated line, so a
//! connection is trivially driven with `nc` for testing.

use std::io;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;

use contract::{Body, Envelope, ErrorCode, Event, ProtocolError};
use engine::Engine;

/// Transport behaviour knobs.
#[derive(Clone)]
pub struct TransportConfig {
    /// Whether `ViewManifest` events are forwarded to the controller. Default
    /// `false`: the on-device view (LCD/TUI) stays on the device and does not
    /// leave over the wire.
    pub forward_view_manifest: bool,
    /// Delay between reconnect attempts in dialer mode.
    pub reconnect_delay: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self { forward_view_manifest: false, reconnect_delay: Duration::from_secs(3) }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Egress policy: what is allowed to leave the device.
fn egress_allows(config: &TransportConfig, env: &Envelope) -> bool {
    match &env.body {
        // Both are on-device UI chrome (the tactical view and the HUD band);
        // they stay on the device unless the operator opts to forward them.
        Body::Event(Event::ViewManifest(_)) | Body::Event(Event::Widget(_)) => {
            config.forward_view_manifest
        }
        _ => true,
    }
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, env: &Envelope) -> io::Result<()> {
    let mut buf =
        serde_json::to_vec(env).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    buf.push(b'\n');
    w.write_all(&buf).await?;
    w.flush().await
}

/// Serve one connection until it closes. A single task reads JSON-line commands
/// into the engine and writes the engine's outbound envelopes back, selecting
/// between the two directions.
pub async fn serve_connection(engine: Arc<Engine>, stream: TcpStream, config: TransportConfig) {
    let (read_half, mut write_half) = stream.into_split();
    // Subscribe before reading any command so no response can be missed.
    let mut rx = engine.subscribe();
    let mut lines = BufReader::new(read_half).lines();
    tracing::debug!("serving connection");

    loop {
        tokio::select! {
            inbound = lines.next_line() => match inbound {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<Envelope>(&line) {
                        Ok(env) => engine.handle(env).await,
                        Err(e) => {
                            let err = Envelope::new(
                                Body::Error(ProtocolError {
                                    code: ErrorCode::InvalidParams,
                                    message: format!("malformed message: {e}"),
                                    module: None,
                                }),
                                now_ms(),
                            );
                            if write_frame(&mut write_half, &err).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                Ok(None) => break, // peer closed the connection
                Err(_) => break,   // read error
            },
            outbound = rx.recv() => match outbound {
                Ok(env) => {
                    if egress_allows(&config, &env)
                        && write_frame(&mut write_half, &env).await.is_err()
                    {
                        break;
                    }
                }
                // Slow consumer fell behind: the receiver already skipped ahead,
                // so resync by dropping the gap and keep going.
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => break,
            },
        }
    }
}

/// Production reverse tunnel: the device dials the controller and keeps
/// reconnecting forever, so a NAT'd drop-box keeps phoning home.
pub async fn run_dialer(engine: Arc<Engine>, addr: String, config: TransportConfig) {
    loop {
        match TcpStream::connect(&addr).await {
            Ok(stream) => {
                tracing::info!(%addr, "reverse tunnel connected");
                serve_connection(engine.clone(), stream, config.clone()).await;
                tracing::warn!(%addr, "reverse tunnel closed; reconnecting");
            }
            Err(e) => tracing::warn!(%addr, "dial failed: {e}; retrying"),
        }
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

/// Local/USB-OTG listener for development: accept inbound connections and serve
/// each on its own task.
pub async fn run_listener(engine: Arc<Engine>, addr: &str, config: TransportConfig) -> io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, peer) = listener.accept().await?;
        tracing::debug!(%peer, "connection accepted");
        let engine = engine.clone();
        let config = config.clone();
        tokio::spawn(async move {
            serve_connection(engine, stream, config).await;
        });
    }
}
