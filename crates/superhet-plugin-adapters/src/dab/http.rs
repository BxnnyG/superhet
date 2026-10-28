//! A minimal HTTP/1.x client for the loopback web server of `welle-cli`.
//!
//! welle-cli answers one request per connection and closes it, so `Connection: close` plus
//! "read until EOF" is all that is needed. No TLS, no redirects, no chunked bodies.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// Largest body [`get`] returns (a slideshow image is at most about 50 kB).
pub const MAX_BODY: usize = 2 * 1024 * 1024;

/// Status line and headers of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// Status code (`200`).
    pub status: u16,
    /// Headers with lower-cased names.
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// Value of header `name` (lower case).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Open a connection, send `GET path` and read the head. The returned reader is positioned at
/// the first byte of the body.
///
/// # Errors
/// Connection, timeout or malformed response.
pub fn open(
    addr: SocketAddr,
    path: &str,
    connect_timeout: Duration,
    read_timeout: Option<Duration>,
) -> io::Result<(Head, BufReader<TcpStream>)> {
    let mut stream = TcpStream::connect_timeout(&addr, connect_timeout)?;
    stream.set_read_timeout(read_timeout)?;
    stream.set_write_timeout(Some(connect_timeout))?;
    write!(
        stream,
        "GET {path} HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\nUser-Agent: superhet-dab\r\n\r\n"
    )?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "malformed status line"))?;
    let mut headers = Vec::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line.trim_end().is_empty() {
            break;
        }
        if headers.len() >= 64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "too many headers",
            ));
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    Ok((Head { status, headers }, reader))
}

/// `GET path` and read the whole body (at most [`MAX_BODY`]).
///
/// # Errors
/// Connection, timeout, malformed response or a body that is too large.
pub fn get(addr: SocketAddr, path: &str, timeout: Duration) -> io::Result<(Head, Vec<u8>)> {
    let (head, reader) = open(addr, path, timeout, Some(timeout))?;
    let mut body = Vec::new();
    reader.take(MAX_BODY as u64 + 1).read_to_end(&mut body)?;
    if body.len() > MAX_BODY {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "body too large"));
    }
    Ok((head, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serve one canned response and return the address.
    fn serve(response: &'static [u8]) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Read the whole request first, or the close would reset the connection.
                let mut seen = Vec::new();
                let mut byte = [0u8; 1];
                while !seen.ends_with(b"\r\n\r\n") && stream.read(&mut byte).is_ok_and(|n| n > 0) {
                    seen.push(byte[0]);
                }
                let _ = stream.write_all(response);
            }
        });
        addr
    }

    #[test]
    fn reads_head_and_body_until_close() {
        let addr =
            serve(b"HTTP/1.0 200 OK\r\nContent-Type: image/png\r\nX-A: b\r\n\r\n\x89PNG\r\nrest");
        let (head, body) = get(addr, "/slide/d210", Duration::from_secs(2)).expect("get");
        assert_eq!(head.status, 200);
        assert_eq!(head.header("content-type"), Some("image/png"));
        assert_eq!(body, b"\x89PNG\r\nrest");
    }

    #[test]
    fn status_codes_and_garbage() {
        let addr = serve(b"HTTP/1.0 404 Not Found\r\n\r\nSlide not available.\r\n");
        let (head, _) = get(addr, "/slide/x", Duration::from_secs(2)).expect("get");
        assert_eq!(head.status, 404);
        let addr = serve(b"nonsense\r\n\r\n");
        assert!(get(addr, "/", Duration::from_secs(2)).is_err());
    }

    #[test]
    fn a_closed_port_is_an_error() {
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr")
        };
        assert!(get(addr, "/", Duration::from_millis(300)).is_err());
    }
}
