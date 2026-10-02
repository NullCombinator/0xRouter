//! The one-shot loopback listener that receives a browser sign-in redirect.
//!
//! Binds the declared `http://127.0.0.1:<port>/…` (or `localhost`, `[::1]`); a missing or
//! zero port means an ephemeral one, written back into the redirect URI. It answers the
//! first `GET <path>?…` with a short page and hands back the query; any other request gets
//! a 404 and the wait goes on. A busy port is reported, not fatal: paste-back still works.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The most a callback request head may take.
const MAX_HEAD: usize = 16 * 1024;
/// How long one connection may take to send its request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

const DONE_PAGE: &str = "<!doctype html><title>Signed in</title><p>Sign-in received. You can close this page and return to the terminal.</p>";
const FAILED_PAGE: &str =
    "<!doctype html><title>Sign-in failed</title><p>Sign-in failed; see the terminal. You can close this page.</p>";

#[derive(Debug, thiserror::Error)]
pub enum BindError {
    #[error("{0} is in use; paste the address from the browser instead")]
    Busy(SocketAddr),
    #[error("{0} is not a loopback redirect")]
    NotLoopback(String),
    #[error("listening on {addr}: {source}")]
    Io { addr: SocketAddr, source: io::Error },
}

#[derive(Debug)]
pub struct Loopback {
    listener: TcpListener,
    path: String,
    redirect_uri: String,
}

impl Loopback {
    /// Binds `redirect_uri`'s address.
    pub async fn bind(redirect_uri: &str) -> Result<Self, BindError> {
        let bad = || BindError::NotLoopback(redirect_uri.to_owned());
        let mut u = url::Url::parse(redirect_uri).map_err(|_| bad())?;
        let ip = match u.host() {
            Some(url::Host::Domain("localhost")) | Some(url::Host::Ipv4(Ipv4Addr::LOCALHOST)) => {
                IpAddr::V4(Ipv4Addr::LOCALHOST)
            }
            Some(url::Host::Ipv6(Ipv6Addr::LOCALHOST)) => IpAddr::V6(Ipv6Addr::LOCALHOST),
            _ => return Err(bad()),
        };
        if u.scheme() != "http" {
            return Err(bad());
        }
        let addr = SocketAddr::new(ip, u.port().unwrap_or(0));
        let listener = TcpListener::bind(addr).await.map_err(|source| match source.kind() {
            io::ErrorKind::AddrInUse => BindError::Busy(addr),
            _ => BindError::Io { addr, source },
        })?;
        if addr.port() == 0 {
            let port = listener.local_addr().map_err(|source| BindError::Io { addr, source })?.port();
            u.set_port(Some(port)).map_err(|()| bad())?;
        }
        Ok(Self { listener, path: u.path().to_owned(), redirect_uri: u.into() })
    }

    /// The redirect URI to send, with the bound port.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Waits for the callback and returns its query string. `verdict` decides the page:
    /// `true` for "signed in". Cancel-safe between connections.
    pub async fn accept(&self, verdict: impl Fn(&str) -> bool) -> io::Result<String> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            if let Ok(Some(query)) = tokio::time::timeout(HEAD_TIMEOUT, self.serve(stream, &verdict)).await {
                return Ok(query);
            }
        }
    }

    /// Reads one request head; answers it; the query when it was the callback.
    async fn serve(&self, mut stream: TcpStream, verdict: &impl Fn(&str) -> bool) -> Option<String> {
        let mut head = Vec::with_capacity(1024);
        let mut buf = [0u8; 2048];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buf).await.ok()?;
            if n == 0 || head.len() + n > MAX_HEAD {
                return None;
            }
            head.extend_from_slice(&buf[..n]);
        }
        let line = head.split(|b| *b == b'\r').next().and_then(|l| std::str::from_utf8(l).ok())?;
        let mut parts = line.split(' ');
        let (method, target) = (parts.next()?, parts.next()?);
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        if method != "GET" || path != self.path {
            let _ = respond(&mut stream, "404 Not Found", "<!doctype html><p>Not found.</p>").await;
            return None;
        }
        let ok = verdict(query);
        let (status, page) = if ok { ("200 OK", DONE_PAGE) } else { ("400 Bad Request", FAILED_PAGE) };
        let _ = respond(&mut stream, status, page).await;
        Some(query.to_owned())
    }
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\ncache-control: no-store\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    stream.shutdown().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answers_the_callback_and_ignores_other_paths() {
        let l = Loopback::bind("http://localhost/callback").await.unwrap();
        let uri = l.redirect_uri().to_owned();
        assert!(uri.starts_with("http://localhost:") && uri.ends_with("/callback"), "{uri}");
        let other = uri.replace("/callback", "/favicon.ico");
        let client = tokio::spawn(async move {
            assert_eq!(reqwest::get(other).await.unwrap().status(), 404);
            let r = reqwest::get(format!("{uri}?code=c&state=s")).await.unwrap();
            (r.status(), r.text().await.unwrap())
        });
        let q = l.accept(|q| q.contains("code=")).await.unwrap();
        assert_eq!(q, "code=c&state=s");
        let (status, page) = client.await.unwrap();
        assert_eq!(status, 200);
        assert!(page.contains("close this page"));
    }

    #[tokio::test]
    async fn a_busy_port_is_reported() {
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = held.local_addr().unwrap().port();
        let err = Loopback::bind(&format!("http://127.0.0.1:{port}/callback")).await.unwrap_err();
        assert!(matches!(err, BindError::Busy(_)), "{err}");
        assert!(err.to_string().contains(&port.to_string()));
        assert!(matches!(Loopback::bind("https://example.com/cb").await, Err(BindError::NotLoopback(_))));
    }
}
