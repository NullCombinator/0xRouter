//! Proxy pausing (spec 013, research R8, data-model § ProxyState).
//!
//! After a connect-class failure through a proxy the engine probes the proxy itself. If the
//! probe fails the proxy is paused: candidates behind it are skipped, with no cooldown, until
//! the operator runs `proxy fixed` and a probe succeeds, or the proxy's definition or an
//! assignment naming it changes. The state lives in `routing/proxies.json` (mode 0600) and
//! holds names, times and reasons only: never an address or a credential.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::proxy::Proxy;
use crate::files;

pub const FILE: &str = "proxies.json";

/// Why and since when a proxy is paused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub since: String,
    pub reason: String,
    /// What the proxy and its assignments looked like when it was paused; in memory only, so
    /// a pause read from the file adopts the first fingerprint it is checked against.
    #[serde(skip)]
    print: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Stored {
    #[serde(default)]
    paused: BTreeMap<String, Pause>,
}

/// The paused proxies, shared by every snapshot.
pub struct ProxyBoard {
    path: PathBuf,
    paused: Mutex<BTreeMap<String, Pause>>,
}

impl ProxyBoard {
    /// Reads `<home>/routing/proxies.json`. A missing or unreadable file means nothing is
    /// paused: a proxy stays in use rather than a request failing on a bad state file.
    pub fn open(home: &Path) -> Self {
        let path = home.join("routing").join(FILE);
        let paused = files::read_private(&path)
            .ok()
            .flatten()
            .and_then(|t| serde_json::from_str::<Stored>(&t).ok())
            .map(|s| s.paused)
            .unwrap_or_default();
        Self { path, paused: Mutex::new(paused) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Pause>> {
        self.paused.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn paused(&self, name: &str) -> Option<Pause> {
        self.lock().get(name).cloned()
    }

    /// Every paused proxy, by name.
    pub fn list(&self) -> Vec<(String, Pause)> {
        self.lock().iter().map(|(n, p)| (n.clone(), p.clone())).collect()
    }

    fn save(&self, paused: &BTreeMap<String, Pause>) {
        let text = serde_json::to_string(&Stored { paused: paused.clone() }).unwrap_or_default();
        if let Err(e) = files::write_private(&self.path, &text) {
            tracing::warn!("proxy pause state not saved: {e}");
        }
    }

    /// Pauses `name` (a no-op when it already is) and warns once.
    pub fn pause(&self, name: &str, reason: &str, print: &str) {
        let mut paused = self.lock();
        if paused.contains_key(name) {
            return;
        }
        let since = crate::clock::now_rfc3339();
        tracing::warn!("proxy {name} paused: {reason}; run `nullrouter proxy fixed {name}` once it is back");
        paused.insert(name.to_owned(), Pause { since, reason: reason.to_owned(), print: Some(print.to_owned()) });
        self.save(&paused);
    }

    /// After a connect-class failure through `proxy`: probes it, and pauses it if the probe
    /// fails. Returns whether it is paused now.
    pub async fn failed(&self, proxy: &Proxy, print: &str, timeout: Duration) -> bool {
        if self.paused(&proxy.name).is_some() {
            return true;
        }
        match probe(proxy, timeout).await {
            Ok(()) => false,
            Err(reason) => {
                self.pause(&proxy.name, &reason, print);
                true
            }
        }
    }

    /// `proxy fixed`: probes `proxy` and clears its pause if it answers. `Err` is why not.
    pub async fn fixed(&self, proxy: &Proxy, timeout: Duration) -> Result<(), String> {
        probe(proxy, timeout).await?;
        let mut paused = self.lock();
        if paused.remove(&proxy.name).is_some() {
            self.save(&paused);
        }
        Ok(())
    }

    /// After a reload: forgets a pause whose proxy is gone, or whose definition or
    /// assignments changed (`prints` is the proxies' current fingerprints, by name).
    pub fn reconcile(&self, prints: &BTreeMap<String, String>) {
        let mut paused = self.lock();
        let before = paused.len();
        paused.retain(|name, p| match (prints.get(name), &p.print) {
            (None, _) => false,
            (Some(now), None) => {
                p.print = Some(now.clone());
                true
            }
            (Some(now), Some(then)) => now == then,
        });
        if paused.len() != before {
            self.save(&paused);
        }
    }
}

/// Reaches the proxy itself, not anything behind it: a TCP connection, for `https` also a
/// TLS handshake, for `socks5` also the greeting. `Err` is the reason for a pause.
pub async fn probe(proxy: &Proxy, timeout: Duration) -> Result<(), String> {
    let url = reqwest::Url::parse(&proxy.url).map_err(|_| "the proxy's address is not a URL".to_owned())?;
    let host = url.host_str().ok_or("the proxy's address has no host")?.to_owned();
    let port = url.port_or_known_default().unwrap_or(1080);
    let work = async {
        let mut tcp = tokio::net::TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| format!("connect to proxy failed: {}", e.kind()))?;
        match url.scheme() {
            "socks5" | "socks5h" => {
                // No authentication offered: a proxy that wants one answers 0xff, still a SOCKS5 server.
                tcp.write_all(&[5, 1, 0]).await.map_err(|e| format!("proxy greeting failed: {}", e.kind()))?;
                let mut reply = [0u8; 2];
                tcp.read_exact(&mut reply).await.map_err(|e| format!("proxy greeting failed: {}", e.kind()))?;
                if reply[0] != 5 {
                    return Err("the address does not answer as a SOCKS5 proxy".to_owned());
                }
            }
            "https" => {
                drop(tcp);
                // Any reply proves the handshake; only a failure to connect or to handshake counts.
                let client = reqwest::Client::builder().no_proxy().build().map_err(|e| e.to_string())?;
                if let Err(e) = client.get(url.as_str()).send().await
                    && e.is_connect()
                {
                    return Err("TLS handshake with the proxy failed".to_owned());
                }
            }
            _ => {}
        }
        Ok(())
    };
    tokio::time::timeout(timeout, work).await.unwrap_or_else(|_| Err(format!("no answer within {} ms", timeout.as_millis())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy(name: &str, url: &str) -> Proxy {
        Proxy { name: name.into(), url: url.into(), username: None, password: None, secret: None }
    }

    #[tokio::test]
    async fn a_dead_port_pauses_and_the_state_survives_a_reopen() {
        let home = tempfile::tempdir().expect("home");
        let board = ProxyBoard::open(home.path());
        let dead = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
            l.local_addr().expect("addr")
        };
        let p = proxy("eu", &format!("http://{dead}"));
        assert!(board.failed(&p, "v1", Duration::from_secs(2)).await);
        assert!(board.paused("eu").is_some());

        let text = std::fs::read_to_string(home.path().join("routing").join(FILE)).expect("file");
        assert!(text.contains("\"eu\"") && !text.contains("127.0.0.1"), "names, times and reasons only: {text}");
        let again = ProxyBoard::open(home.path());
        assert!(again.paused("eu").is_some());
    }

    #[tokio::test]
    async fn fixed_clears_only_when_the_probe_answers() {
        let home = tempfile::tempdir().expect("home");
        let board = ProxyBoard::open(home.path());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let up = proxy("eu", &format!("http://{}", listener.local_addr().expect("addr")));
        board.pause("eu", "was down", "v1");
        board.fixed(&up, Duration::from_secs(2)).await.expect("reachable");
        assert!(board.paused("eu").is_none());

        let down = proxy("eu", "http://127.0.0.1:1");
        board.pause("eu", "down", "v1");
        assert!(board.fixed(&down, Duration::from_secs(2)).await.is_err());
        assert!(board.paused("eu").is_some());
    }

    #[test]
    fn a_changed_definition_or_a_removed_proxy_clears_the_pause() {
        let home = tempfile::tempdir().expect("home");
        let board = ProxyBoard::open(home.path());
        board.pause("a", "down", "v1");
        board.pause("b", "down", "v1");
        board.pause("c", "down", "v1");
        let prints: BTreeMap<String, String> =
            [("a".to_owned(), "v1".to_owned()), ("b".to_owned(), "v2".to_owned())].into();
        board.reconcile(&prints);
        assert!(board.paused("a").is_some());
        assert!(board.paused("b").is_none(), "changed");
        assert!(board.paused("c").is_none(), "removed");
    }

    #[test]
    fn a_pause_read_from_the_file_adopts_the_first_fingerprint() {
        let home = tempfile::tempdir().expect("home");
        ProxyBoard::open(home.path()).pause("a", "down", "v1");
        let board = ProxyBoard::open(home.path());
        board.reconcile(&[("a".to_owned(), "v9".to_owned())].into());
        assert!(board.paused("a").is_some());
        board.reconcile(&[("a".to_owned(), "v10".to_owned())].into());
        assert!(board.paused("a").is_none());
    }
}
