//! In-process scripted JSON-RPC server for client tests.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};

/// Tiny scripted JSON-RPC server: dispatches on `method` via the provided
/// handler until dropped. A handler result containing the key
/// `__jsonrpc_error` is sent as a JSON-RPC error envelope instead of a
/// result.
pub(crate) struct FakeNode {
    pub(crate) url: String,
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl FakeNode {
    pub(crate) fn start<F>(handler: F) -> Self
    where
        F: Fn(&str, &Value) -> Value + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("addr");
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_flag = shutdown.clone();

        let handle = std::thread::spawn(move || {
            while !shutdown_flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).expect("blocking stream");
                        let Some(request) = read_http_request(&mut stream) else {
                            continue;
                        };
                        let parsed: Value = serde_json::from_str(&request).unwrap_or(Value::Null);
                        let method = parsed
                            .get("method")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let params = parsed.get("params").cloned().unwrap_or(Value::Null);
                        let result = handler(&method, &params);
                        let body = if result.get("__jsonrpc_error").is_some() {
                            json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "error": result["__jsonrpc_error"],
                            })
                            .to_string()
                        } else {
                            json!({
                                "jsonrpc": "2.0",
                                "id": 1,
                                "result": result,
                            })
                            .to_string()
                        };
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            url: format!("http://{addr}"),
            shutdown,
            handle: Some(handle),
        }
    }
}

impl Drop for FakeNode {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> Option<String> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut data = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..header_end]).to_ascii_lowercase();
                    let content_len = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if data.len() >= header_end + 4 + content_len {
                        return Some(String::from_utf8_lossy(&data[header_end + 4..]).into_owned());
                    }
                }
            }
            Err(_) => break,
        }
    }
    None
}
