//! Native bounded WebSocket transport.
//!
//! The public service performs the exact URL allowlist check before this
//! transport is called. Connections are owned by worker threads so WebView
//! dispatch never blocks on network reads. This first transport intentionally
//! supports `ws://` only; TLS URLs remain unavailable until a pinned TLS
//! policy and certificate strategy are added.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};

use kiri_core::error::{Error, Result};
use kiri_core::websocket::{WsBackend, WsMessage};

enum Command {
    Send(String),
    Close,
}

const CHANNEL_CAPACITY: usize = 64;

struct ActiveConnection {
    commands: SyncSender<Command>,
    inbound: Receiver<WsMessage>,
}

pub struct NativeWsBackend {
    next_id: AtomicU64,
    active: Mutex<HashMap<u64, ActiveConnection>>,
}

impl NativeWsBackend {
    pub fn new() -> Self {
        Self { next_id: AtomicU64::new(1), active: Mutex::new(HashMap::new()) }
    }
}

impl Default for NativeWsBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn worker(
    mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
    commands: Receiver<Command>,
    inbound: SyncSender<WsMessage>,
) {
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        let _ = stream.set_nonblocking(true);
    }
    loop {
        match commands.recv_timeout(Duration::from_millis(10)) {
            Ok(Command::Send(message)) => {
                if socket.send(Message::Text(message.into())).is_err() {
                    return;
                }
            }
            Ok(Command::Close) => {
                let _ = socket.close(None);
                return;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        match socket.read() {
            Ok(Message::Text(message)) => {
                match inbound.try_send(WsMessage {
                    direction: "in".to_string(),
                    payload: message.to_string(),
                }) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            Ok(Message::Binary(_))
            | Ok(Message::Ping(_))
            | Ok(Message::Pong(_))
            | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(_)) => return,
            Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => return,
        }
    }
}

impl WsBackend for NativeWsBackend {
    fn open(&self, url: &str) -> Result<u64> {
        if !url.starts_with("ws://") {
            return Err(Error::service_unavailable(
                "kiri.ws currently supports ws:// only; TLS transport is not enabled",
            ));
        }
        let (socket, _) = connect(url)
            .map_err(|e| Error::service_unavailable(format!("kiri.ws.connect failed: {e}")))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (commands, command_rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        let (inbound_tx, inbound_rx) = mpsc::sync_channel(CHANNEL_CAPACITY);
        thread::Builder::new()
            .name(format!("kiri-ws-{id}"))
            .spawn(move || worker(socket, command_rx, inbound_tx))
            .map_err(|e| Error::service_unavailable(format!("kiri.ws worker failed: {e}")))?;
        self.active
            .lock()
            .map_err(|_| Error::command_error("kiri.ws state poisoned"))?
            .insert(id, ActiveConnection { commands, inbound: inbound_rx });
        Ok(id)
    }

    fn send(&self, conn_id: u64, message: &str) -> Result<()> {
        let active =
            self.active.lock().map_err(|_| Error::command_error("kiri.ws state poisoned"))?;
        active
            .get(&conn_id)
            .ok_or_else(|| {
                Error::resource_not_found(format!("kiri.ws unknown connection {conn_id}"))
            })?
            .commands
            .try_send(Command::Send(message.to_string()))
            .map_err(|error| match error {
                TrySendError::Full(_) => Error::busy("kiri.ws outbound queue is full"),
                TrySendError::Disconnected(_) => {
                    Error::service_unavailable("kiri.ws connection closed")
                }
            })
    }

    fn close(&self, conn_id: u64) -> Result<()> {
        let mut active =
            self.active.lock().map_err(|_| Error::command_error("kiri.ws state poisoned"))?;
        let connection = active.remove(&conn_id).ok_or_else(|| {
            Error::resource_not_found(format!("kiri.ws unknown connection {conn_id}"))
        })?;
        // A peer may have already closed the socket and dropped the worker's
        // receiver. Local close is still complete in that state because the
        // host has removed the connection from its table.
        let _ = connection.commands.try_send(Command::Close);
        Ok(())
    }

    fn drain(&self, conn_id: u64) -> Vec<WsMessage> {
        let Ok(active) = self.active.lock() else { return Vec::new() };
        let Some(connection) = active.get(&conn_id) else { return Vec::new() };
        let mut out = Vec::new();
        while let Ok(message) = connection.inbound.try_recv() {
            out.push(message);
            if out.len() >= 256 {
                break;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn transport_queues_are_bounded_and_report_saturation() {
        let (sender, _receiver) = mpsc::sync_channel::<Command>(1);
        sender.try_send(Command::Send("one".to_string())).expect("first slot");
        assert!(matches!(
            sender.try_send(Command::Send("two".to_string())),
            Err(TrySendError::Full(_))
        ));
    }

    #[test]
    fn loopback_server_roundtrips_a_text_message() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept websocket");
            let mut socket = tungstenite::accept(stream).expect("websocket handshake");
            if let Ok(Message::Text(message)) = socket.read() {
                socket.send(Message::Text(message)).expect("echo websocket message");
            }
        });

        let backend = NativeWsBackend::new();
        let id = backend.open(&format!("ws://127.0.0.1:{port}")).expect("connect loopback");
        backend.send(id, "hello").expect("send loopback");
        let mut received = Vec::new();
        for _ in 0..50 {
            received.extend(backend.drain(id));
            if !received.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(received.first().map(|m| m.payload.as_str()), Some("hello"));
        backend.close(id).expect("close loopback");
        server.join().expect("server thread");
    }
}
