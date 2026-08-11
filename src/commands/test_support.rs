use std::io::Read;
use std::net::TcpStream;
use std::time::Duration;

/// Read one HTTP request through its declared body before a test stub responds.
///
/// Consuming the complete request prevents the stub from closing a socket with
/// unread inbound data, which can turn the close into an RST and discard a
/// response that was already written.
pub(crate) fn drain_http_request(stream: &mut TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                request.extend_from_slice(&buf[..n]);
                if http_request_complete(&request) {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break
            }
            Err(err) => panic!("read request: {err}"),
        }
    }
}

fn http_request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
    let content_len = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_len
}
