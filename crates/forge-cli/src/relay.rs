//! `forge relay --port N`: a localhost port on this machine whose
//! connections are carried over stdin/stdout to whoever started the relay.
//!
//! A desktop app running ForgeCLI on a machine it reaches only through a
//! process channel (`wsl.exe`, `docker exec -i`) starts a relay beside it.
//! ForgeCLI there connects to `127.0.0.1:N` as if the service were local;
//! the app, on the other end of the pipe, joins each connection to the
//! service on its own machine (its model proxy, its browser tools).
//!
//! Frames, both ways: `kind: u8`, `id: u32` (big-endian), `len: u32`, then
//! `len` bytes.
//!
//! - `0` ready (relay to app, once, after the port is bound)
//! - `1` open (relay to app: connection `id` arrived)
//! - `2` data (either way)
//! - `3` close (either way: that side is done with `id`)
//!
//! The relay ends when its stdin closes.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex};

use crate::exit::{self, Fail};

pub const READY: u8 = 0;
pub const OPEN: u8 = 1;
pub const DATA: u8 = 2;
pub const CLOSE: u8 = 3;

/// The largest payload in one frame.
const CHUNK: usize = 32 * 1024;

pub fn frame(kind: u8, id: u32, data: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(9 + data.len());
    f.push(kind);
    f.extend_from_slice(&id.to_be_bytes());
    f.extend_from_slice(&(data.len() as u32).to_be_bytes());
    f.extend_from_slice(data);
    f
}

type Conns = Arc<Mutex<HashMap<u32, mpsc::UnboundedSender<Option<Vec<u8>>>>>>;

pub async fn run(port: u16) -> Result<i32, Fail> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| Fail::config(format!("relay: port {port} is not free here: {e}")))?;
    // Everything for stdout goes through one writer, frame by frame.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(f) = out_rx.recv().await {
            if stdout.write_all(&f).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let _ = out_tx.send(frame(READY, 0, &[]));
    let conns: Conns = Arc::new(Mutex::new(HashMap::new()));

    // Connections arriving here.
    let (acc_conns, acc_out) = (conns.clone(), out_tx.clone());
    let accept = tokio::spawn(async move {
        let mut next: u32 = 1;
        while let Ok((sock, _)) = listener.accept().await {
            let id = next;
            next = next.wrapping_add(1).max(1);
            let (mut rd, mut wr) = sock.into_split();
            let (tx, mut rx) = mpsc::unbounded_channel::<Option<Vec<u8>>>();
            acc_conns.lock().await.insert(id, tx);
            let _ = acc_out.send(frame(OPEN, id, &[]));
            // Bytes from the app to this connection; None: the app closed it.
            tokio::spawn(async move {
                while let Some(Some(data)) = rx.recv().await {
                    if wr.write_all(&data).await.is_err() {
                        break;
                    }
                }
                let _ = wr.shutdown().await;
            });
            // Bytes from this connection to the app.
            let out = acc_out.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; CHUNK];
                loop {
                    match rd.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if out.send(frame(DATA, id, &buf[..n])).is_err() {
                                break;
                            }
                        }
                    }
                }
                let _ = out.send(frame(CLOSE, id, &[]));
            });
        }
    });

    // Frames from the app.
    let mut stdin = tokio::io::stdin();
    let mut head = [0u8; 9];
    loop {
        if stdin.read_exact(&mut head).await.is_err() {
            break;
        }
        let kind = head[0];
        let id = u32::from_be_bytes([head[1], head[2], head[3], head[4]]);
        let len = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) as usize;
        if len > 16 * CHUNK {
            break;
        }
        let mut data = vec![0u8; len];
        if stdin.read_exact(&mut data).await.is_err() {
            break;
        }
        match kind {
            DATA => {
                if let Some(tx) = conns.lock().await.get(&id) {
                    let _ = tx.send(Some(data));
                }
            }
            CLOSE => {
                if let Some(tx) = conns.lock().await.remove(&id) {
                    let _ = tx.send(None);
                }
            }
            _ => {}
        }
    }
    accept.abort();
    drop(out_tx);
    writer.abort();
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_kind_id_length_payload() {
        assert_eq!(frame(DATA, 258, b"hi"), [2, 0, 0, 1, 2, 0, 0, 0, 2, b'h', b'i']);
        assert_eq!(frame(READY, 0, &[]), [0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }
}
