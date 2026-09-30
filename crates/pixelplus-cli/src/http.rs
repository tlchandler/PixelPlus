//! A deliberately tiny HTTP/1.1 GET client for talking to the local daemon
//! (plain HTTP on the loopback interface; no TLS, no proxies).

use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// A parsed `http://host[:port][/prefix]` base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl {
    /// Host name or address.
    pub host: String,
    /// TCP port.
    pub port: u16,
    /// Path prefix without a trailing slash.
    pub prefix: String,
}

impl BaseUrl {
    /// Parse a base URL.
    pub fn parse(url: &str) -> Result<BaseUrl> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| anyhow!("only http:// URLs are supported (got `{url}`)"))?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.is_empty() {
            bail!("URL `{url}` has no host");
        }
        let parse_port =
            |p: &str| -> Result<u16> { p.parse().with_context(|| format!("bad port in `{url}`")) };
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, after) = v6
                .split_once(']')
                .ok_or_else(|| anyhow!("unterminated IPv6 address in `{url}`"))?;
            let port = match after.strip_prefix(':') {
                Some(p) => parse_port(p)?,
                None if after.is_empty() => 80,
                None => bail!("unexpected `{after}` after the host in `{url}`"),
            };
            (h.to_string(), port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), parse_port(p)?),
                None => (authority.to_string(), 80),
            }
        };
        if host.is_empty() {
            bail!("URL `{url}` has no host");
        }
        Ok(BaseUrl {
            host,
            port,
            prefix: path.trim_end_matches('/').to_string(),
        })
    }
}

/// A response.
#[derive(Debug, Clone)]
pub struct Response {
    /// HTTP status code.
    pub status: u16,
    /// Decoded body.
    pub body: Vec<u8>,
}

impl Response {
    /// Parse the body as JSON.
    pub fn json(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body).context("the daemon returned invalid JSON")
    }
}

/// `GET base + path`, with a connect/read timeout.
pub fn get(base: &BaseUrl, path: &str, timeout: Duration) -> Result<Response> {
    let addr = (base.host.as_str(), base.port)
        .to_socket_addrs()
        .with_context(|| format!("resolving {}", base.host))?
        .next()
        .ok_or_else(|| anyhow!("{} has no address", base.host))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("connecting to {}:{}", base.host, base.port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = format!(
        "GET {}{} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nUser-Agent: pixelplus-cli/{}\r\nConnection: close\r\n\r\n",
        base.prefix,
        path,
        base.host,
        env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream
        .take(16 * 1024 * 1024)
        .read_to_end(&mut raw)
        .context("reading the response")?;
    parse_response(&raw)
}

/// Parse a raw HTTP/1.x response (Content-Length, chunked, or read-to-close).
pub fn parse_response(raw: &[u8]) -> Result<Response> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("malformed HTTP response"))?;
    let head = std::str::from_utf8(&raw[..split]).context("non-UTF-8 HTTP headers")?;
    let body = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("malformed status line `{status_line}`"))?;
    let mut chunked = false;
    let mut length: Option<usize> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
            if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            } else if k == "content-length" {
                length = v.parse().ok();
            }
        }
    }
    let body = if chunked {
        dechunk(body)?
    } else if let Some(n) = length {
        body.get(..n)
            .ok_or_else(|| anyhow!("response body shorter than Content-Length"))?
            .to_vec()
    } else {
        body.to_vec()
    };
    Ok(Response { status, body })
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| anyhow!("malformed chunked body"))?;
        let size_str = std::str::from_utf8(&data[..eol])?;
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16)
            .with_context(|| format!("bad chunk size `{size_hex}`"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        let chunk = data.get(..size).ok_or_else(|| anyhow!("truncated chunk"))?;
        out.extend_from_slice(chunk);
        data = data.get(size + 2..).unwrap_or(&[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parsing() {
        let u = BaseUrl::parse("http://127.0.0.1").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.prefix.as_str()),
            ("127.0.0.1", 80, "")
        );
        let u = BaseUrl::parse("http://pi.local:8080/x/").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.prefix.as_str()),
            ("pi.local", 8080, "/x")
        );
        let u = BaseUrl::parse("http://[::1]:81").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 81));
        let u = BaseUrl::parse("http://[::1]").unwrap();
        assert_eq!((u.host.as_str(), u.port), ("::1", 80));
        assert!(BaseUrl::parse("http://[::1").is_err());
        assert!(BaseUrl::parse("https://x").is_err());
        assert!(BaseUrl::parse("http://").is_err());
        assert!(BaseUrl::parse("http://h:notaport").is_err());
    }

    #[test]
    fn responses() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}extra").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"{}"[..]));
        let r = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3;x=y\r\n:1}\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.json().unwrap()["a"], 1);
        let r = parse_response(b"HTTP/1.1 401 Unauthorized\r\n\r\nno").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (401, &b"no"[..]));
        assert!(parse_response(b"garbage").is_err());
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n{}").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_err()
        );
    }

    #[test]
    fn unreachable_daemon_is_an_error() {
        // Port 9 (discard) on loopback is essentially never listening.
        let base = BaseUrl::parse("http://127.0.0.1:9").unwrap();
        assert!(get(&base, "/api/v1/system", Duration::from_millis(300)).is_err());
    }
}
