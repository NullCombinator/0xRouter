//! A minimal forward proxy on `127.0.0.1` (spec 013): HTTP `CONNECT`, absolute-form HTTP
//! requests and SOCKS5, each with optional credentials. It counts the connections it
//! carried, and can be stopped and started again on the same port, so a test can take a
//! proxy down and bring it back.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

/// The credentials a proxy asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credentials {
    pub user: String,
    pub password: String,
}

#[derive(Default)]
struct Counts {
    /// Connections the proxy carried to a target.
    carried: AtomicUsize,
    /// Clients refused for missing or wrong credentials.
    refused: AtomicUsize,
}

pub struct MockProxy {
    addr: SocketAddr,
    credentials: Option<Credentials>,
    counts: Arc<Counts>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for MockProxy {
    fn drop(&mut self) {
        if let Some(t) = self.task.get_mut().unwrap_or_else(|e| e.into_inner()).take() {
            t.abort();
        }
    }
}

impl MockProxy {
    /// A proxy that asks for no credentials.
    pub async fn start() -> Self {
        Self::bind(None).await
    }

    /// A proxy that wants `user` and `password` (basic auth for HTTP, RFC 1929 for SOCKS5).
    pub async fn start_with_auth(user: &str, password: &str) -> Self {
        Self::bind(Some(Credentials { user: user.into(), password: password.into() })).await
    }

    async fn bind(credentials: Option<Credentials>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let counts = Arc::new(Counts::default());
        let task = spawn(listener, credentials.clone(), counts.clone());
        Self { addr, credentials, counts, task: Mutex::new(Some(task)) }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<port>`, with the credentials when there are some.
    pub fn url(&self) -> String {
        self.url_with("http")
    }

    /// `socks5://127.0.0.1:<port>`.
    pub fn socks_url(&self) -> String {
        self.url_with("socks5")
    }

    fn url_with(&self, scheme: &str) -> String {
        match &self.credentials {
            Some(c) => format!("{scheme}://{}:{}@{}", c.user, c.password, self.addr),
            None => format!("{scheme}://{}", self.addr),
        }
    }

    /// Connections carried to a target since the start (not reset by `stop`).
    pub fn carried(&self) -> usize {
        self.counts.carried.load(Ordering::SeqCst)
    }

    /// Clients turned away for missing or wrong credentials.
    pub fn refused(&self) -> usize {
        self.counts.refused.load(Ordering::SeqCst)
    }

    /// Closes the listener and every connection it carries: the port refuses connections.
    pub async fn stop(&self) {
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(t) = task {
            t.abort();
            let _ = t.await;
        }
    }

    /// Listens again on the same port.
    pub async fn start_again(&self) {
        if self.task.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return;
        }
        let mut listener = None;
        for _ in 0..50 {
            match TcpListener::bind(self.addr).await {
                Ok(l) => {
                    listener = Some(l);
                    break;
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
        let listener = listener.expect("rebind the proxy's port");
        let task = spawn(listener, self.credentials.clone(), self.counts.clone());
        *self.task.lock().unwrap_or_else(|e| e.into_inner()) = Some(task);
    }
}

fn spawn(listener: TcpListener, credentials: Option<Credentials>, counts: Arc<Counts>) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Dropping the set on abort closes every carried connection with the listener.
        let mut conns = JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else { break };
            let (credentials, counts) = (credentials.clone(), counts.clone());
            conns.spawn(async move {
                let _ = serve(stream, credentials, counts).await;
            });
        }
    })
}

async fn serve(mut client: TcpStream, credentials: Option<Credentials>, counts: Arc<Counts>) -> std::io::Result<()> {
    let mut first = [0u8; 1];
    if client.peek(&mut first).await? == 0 {
        return Ok(());
    }
    if first[0] == 0x05 {
        socks5(client, credentials, counts).await
    } else {
        http(&mut client, credentials, counts).await
    }
}

fn basic(c: &Credentials) -> String {
    use base64::Engine;
    format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", c.user, c.password)))
}

async fn http(client: &mut TcpStream, credentials: Option<Credentials>, counts: Arc<Counts>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let end = loop {
        let mut chunk = [0u8; 2048];
        let n = client.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let mut parts = request_line.split(' ');
    let (method, target, version) =
        (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or("HTTP/1.1"));
    let headers: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();

    if let Some(c) = &credentials {
        let want = basic(c);
        let given = headers
            .iter()
            .find_map(|h| h.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("proxy-authorization")))
            .map(|(_, v)| v.trim());
        if given != Some(want.as_str()) {
            counts.refused.fetch_add(1, Ordering::SeqCst);
            client
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nproxy-authenticate: Basic realm=\"mock\"\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await?;
            return Ok(());
        }
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let Ok(mut upstream) = TcpStream::connect(target).await else {
            client.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n").await?;
            return Ok(());
        };
        counts.carried.fetch_add(1, Ordering::SeqCst);
        client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        // Bytes the client sent after its head belong to the tunnel.
        upstream.write_all(&buf[end..]).await?;
        tokio::io::copy_bidirectional(client, &mut upstream).await?;
        return Ok(());
    }

    // Absolute-form (`GET http://host:port/path`): forward in origin-form.
    let Some(rest) = target.strip_prefix("http://") else {
        client.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n").await?;
        return Ok(());
    };
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    let dial = if authority.contains(':') { authority.to_owned() } else { format!("{authority}:80") };
    let Ok(mut upstream) = TcpStream::connect(&dial).await else {
        client.write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\n\r\n").await?;
        return Ok(());
    };
    counts.carried.fetch_add(1, Ordering::SeqCst);
    let mut out = format!("{method} {path} {version}\r\n");
    for h in headers {
        let name = h.split_once(':').map_or("", |(k, _)| k);
        if !name.eq_ignore_ascii_case("proxy-authorization") && !name.eq_ignore_ascii_case("proxy-connection") {
            out.push_str(h);
            out.push_str("\r\n");
        }
    }
    out.push_str("\r\n");
    upstream.write_all(out.as_bytes()).await?;
    upstream.write_all(&buf[end..]).await?;
    tokio::io::copy_bidirectional(client, &mut upstream).await?;
    Ok(())
}

async fn socks5(
    mut client: TcpStream,
    credentials: Option<Credentials>,
    counts: Arc<Counts>,
) -> std::io::Result<()> {
    // Greeting: VER NMETHODS METHODS...
    let mut head = [0u8; 2];
    client.read_exact(&mut head).await?;
    let mut methods = vec![0u8; usize::from(head[1])];
    client.read_exact(&mut methods).await?;
    match &credentials {
        None => client.write_all(&[5, 0]).await?,
        Some(c) => {
            if !methods.contains(&2) {
                counts.refused.fetch_add(1, Ordering::SeqCst);
                client.write_all(&[5, 0xff]).await?;
                return Ok(());
            }
            client.write_all(&[5, 2]).await?;
            // RFC 1929: VER ULEN UNAME PLEN PASSWD
            let mut v = [0u8; 2];
            client.read_exact(&mut v).await?;
            let mut user = vec![0u8; usize::from(v[1])];
            client.read_exact(&mut user).await?;
            let mut plen = [0u8; 1];
            client.read_exact(&mut plen).await?;
            let mut pass = vec![0u8; usize::from(plen[0])];
            client.read_exact(&mut pass).await?;
            if user != c.user.as_bytes() || pass != c.password.as_bytes() {
                counts.refused.fetch_add(1, Ordering::SeqCst);
                client.write_all(&[1, 1]).await?;
                return Ok(());
            }
            client.write_all(&[1, 0]).await?;
        }
    }
    // Request: VER CMD RSV ATYP ADDR PORT
    let mut req = [0u8; 4];
    client.read_exact(&mut req).await?;
    let host = match req[3] {
        1 => {
            let mut a = [0u8; 4];
            client.read_exact(&mut a).await?;
            std::net::Ipv4Addr::from(a).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            client.read_exact(&mut len).await?;
            let mut name = vec![0u8; usize::from(len[0])];
            client.read_exact(&mut name).await?;
            String::from_utf8_lossy(&name).into_owned()
        }
        4 => {
            let mut a = [0u8; 16];
            client.read_exact(&mut a).await?;
            format!("[{}]", std::net::Ipv6Addr::from(a))
        }
        _ => {
            client.write_all(&[5, 8, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
            return Ok(());
        }
    };
    let mut port = [0u8; 2];
    client.read_exact(&mut port).await?;
    let Ok(mut upstream) = TcpStream::connect(format!("{host}:{}", u16::from_be_bytes(port))).await else {
        client.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
        return Ok(());
    };
    counts.carried.fetch_add(1, Ordering::SeqCst);
    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{MockUpstream, Step};

    fn client(proxy: &str) -> reqwest::Client {
        reqwest::Client::builder().proxy(reqwest::Proxy::all(proxy).expect("proxy")).build().expect("client")
    }

    #[tokio::test]
    async fn http_and_socks_requests_reach_the_upstream_through_it() {
        let up = MockUpstream::start().await;
        up.respond(|_| Step::json(200, serde_json::json!({"ok": true})));
        let proxy = MockProxy::start().await;
        for url in [proxy.url(), proxy.socks_url()] {
            let r = client(&url).get(up.url("/x")).send().await.expect("through the proxy");
            assert_eq!(r.status(), 200);
        }
        assert_eq!(proxy.carried(), 2);
        assert_eq!(up.direct_connections(&proxy), 0);
    }

    #[tokio::test]
    async fn credentials_are_required_and_a_stopped_proxy_refuses() {
        let up = MockUpstream::start().await;
        up.respond(|_| Step::json(200, serde_json::json!({})));
        let proxy = MockProxy::start_with_auth("u", "p").await;
        let bare = format!("http://{}", proxy.addr());
        let r = client(&bare).get(up.url("/x")).send().await.expect("407 is a reply");
        assert_eq!(r.status(), 407);
        assert_eq!(proxy.refused(), 1);
        assert_eq!(client(&proxy.url()).get(up.url("/x")).send().await.expect("auth").status(), 200);

        proxy.stop().await;
        assert!(client(&proxy.url()).get(up.url("/x")).send().await.is_err());
        proxy.start_again().await;
        assert_eq!(client(&proxy.url()).get(up.url("/x")).send().await.expect("back").status(), 200);
    }
}
