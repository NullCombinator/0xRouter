//! The one-shot loopback listener that receives a browser sign-in redirect.
//!
//! Binds the declared `http://127.0.0.1:<port>/…` (or `localhost`, `[::1]`); a missing or
//! zero port means an ephemeral one, written back into the redirect URI. It answers the
//! first `GET <path>?…` with a short page and hands back the query; any other request gets
//! a 404 and the wait goes on. A busy port is reported, not fatal: paste-back still works.
//!
//! A `localhost` redirect binds the port on both loopback families, `127.0.0.1` and
//! `[::1]` (security review L7): a browser may resolve `localhost` to either, and a port
//! left free on one family could be taken by another local user's listener, which would
//! then receive the code and state. The redirect URI stays `localhost`, as the provider
//! registered it. Where the system has no IPv6 loopback, `127.0.0.1` alone is bound.

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
    /// One listener, or two for `localhost` (IPv4 and IPv6 on the same port).
    listeners: Vec<TcpListener>,
    path: String,
    redirect_uri: String,
}

/// How many ephemeral ports are tried for a `localhost` redirect whose IPv6 twin is taken.
const LOCALHOST_TRIES: usize = 8;

async fn bind_one(addr: SocketAddr) -> Result<TcpListener, BindError> {
    TcpListener::bind(addr).await.map_err(|source| match source.kind() {
        io::ErrorKind::AddrInUse => BindError::Busy(addr),
        _ => BindError::Io { addr, source },
    })
}

/// `127.0.0.1:port` and `[::1]` on the same port; `[::1]` is skipped only where the
/// system has no IPv6 loopback (nobody can listen there either).
async fn bind_localhost(port: u16) -> Result<Vec<TcpListener>, BindError> {
    for _ in 0..LOCALHOST_TRIES {
        let v4 = bind_one(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)).await?;
        let bound = v4
            .local_addr()
            .map_err(|source| BindError::Io { addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port), source })?
            .port();
        match bind_one(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bound)).await {
            Ok(v6) => return Ok(vec![v4, v6]),
            // An ephemeral port whose IPv6 twin is taken: try another.
            Err(BindError::Busy(_)) if port == 0 => continue,
            Err(e @ BindError::Busy(_)) => return Err(e),
            Err(e) => {
                tracing::debug!(error = %e, "no IPv6 loopback; the localhost redirect listens on 127.0.0.1 only");
                return Ok(vec![v4]);
            }
        }
    }
    Err(BindError::Busy(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port)))
}

impl Loopback {
    /// Binds `redirect_uri`'s address.
    pub async fn bind(redirect_uri: &str) -> Result<Self, BindError> {
        let bad = || BindError::NotLoopback(redirect_uri.to_owned());
        let mut u = url::Url::parse(redirect_uri).map_err(|_| bad())?;
        let ip = match u.host() {
            Some(url::Host::Domain("localhost")) => None,
            Some(url::Host::Ipv4(Ipv4Addr::LOCALHOST)) => Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            Some(url::Host::Ipv6(Ipv6Addr::LOCALHOST)) => Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
            _ => return Err(bad()),
        };
        if u.scheme() != "http" {
            return Err(bad());
        }
        let port = u.port().unwrap_or(0);
        let listeners = match ip {
            Some(ip) => vec![bind_one(SocketAddr::new(ip, port)).await?],
            None => bind_localhost(port).await?,
        };
        if port == 0 {
            let addr = SocketAddr::new(ip.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)), 0);
            let bound = listeners[0].local_addr().map_err(|source| BindError::Io { addr, source })?.port();
            u.set_port(Some(bound)).map_err(|()| bad())?;
        }
        Ok(Self { listeners, path: u.path().to_owned(), redirect_uri: u.into() })
    }

    /// The redirect URI to send, with the bound port.
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Waits for the callback and returns its query string. `verdict` decides the page:
    /// `true` for "signed in". Cancel-safe between connections.
    pub async fn accept(&self, verdict: impl Fn(&str) -> bool) -> io::Result<String> {
        loop {
            let stream = self.accept_any().await?;
            if let Ok(Some(query)) = tokio::time::timeout(HEAD_TIMEOUT, self.serve(stream, &verdict)).await {
                return Ok(query);
            }
        }
    }

    /// The next connection on any of the listeners. Cancel-safe.
    async fn accept_any(&self) -> io::Result<TcpStream> {
        match self.listeners.as_slice() {
            [one] => one.accept().await.map(|(s, _)| s),
            [a, b, ..] => tokio::select! {
                r = a.accept() => r.map(|(s, _)| s),
                r = b.accept() => r.map(|(s, _)| s),
            },
            [] => std::future::pending().await,
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

    /// L7: `localhost` listens on both loopback families, so neither can be taken by
    /// another listener; a fixed port whose IPv6 twin is taken is busy.
    #[tokio::test]
    async fn localhost_binds_both_loopback_families() {
        let Ok(held) = std::net::TcpListener::bind("[::1]:0") else {
            eprintln!("no IPv6 loopback here; skipped");
            return;
        };
        let port = held.local_addr().unwrap().port();
        let err = Loopback::bind(&format!("http://localhost:{port}/callback")).await.unwrap_err();
        assert!(matches!(err, BindError::Busy(a) if a.is_ipv6()), "{err}");
        drop(held);

        let l = Loopback::bind("http://localhost:0/callback").await.unwrap();
        let port: u16 = url::Url::parse(l.redirect_uri()).unwrap().port().unwrap();
        assert!(std::net::TcpListener::bind(("::1", port)).is_err(), "[::1]:{port} is ours too");
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err(), "127.0.0.1:{port} is ours");
        let client = tokio::spawn(async move {
            reqwest::get(format!("http://[::1]:{port}/callback?code=c&state=s")).await.unwrap().status()
        });
        assert_eq!(l.accept(|_| true).await.unwrap(), "code=c&state=s", "answered over IPv6");
        assert_eq!(client.await.unwrap(), 200);
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
