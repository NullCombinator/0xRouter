//! The HTTP version a client speaks (spec 013, US5): `Negotiate` takes HTTP/2 from a server that
//! offers it, `Http1Only` stays on HTTP/1.1 against the same server, and `Negotiate` against a
//! server that offers only HTTP/1.1 is not forced to anything else. The server is a local TLS
//! server with a self-signed certificate; the clients are the engine's own, built by `Clients`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::service::service_fn;
use hyper::{Request, Response, Version};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use nullrouter_engine::connection::clients::{ClientKey, Clients, HttpMode};
use nullrouter_engine::connection::proxy::Proxies;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// A TLS server whose ALPN offer is `alpn`.
async fn tls_server(alpn: &[&[u8]]) -> SocketAddr {
    let key = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string()]).unwrap();
    let cert = CertificateDer::from(key.cert.der().to_vec());
    let pk = PrivateKeyDer::try_from(key.key_pair.serialize_der()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert], pk)
        .unwrap();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else { return };
                let svc = service_fn(|_req: Request<Incoming>| async {
                    Ok::<_, Infallible>(Response::new(Full::new(Bytes::from("ok"))))
                });
                let _ = auto::Builder::new(TokioExecutor::new()).serve_connection(TokioIo::new(tls), svc).await;
            });
        }
    });
    addr
}

async fn version(server: SocketAddr, http: HttpMode) -> Version {
    let clients = Clients::new(true, Proxies::default()).trusting_any_certificate();
    let client = clients.get(&ClientKey { proxy: None, http, reuse: true }).unwrap();
    client.get(format!("https://{server}/")).send().await.unwrap().version()
}

#[tokio::test]
async fn negotiation_takes_http2_and_http1_only_does_not() {
    let both = tls_server(&[b"h2", b"http/1.1"]).await;
    assert_eq!(version(both, HttpMode::Negotiate).await, Version::HTTP_2);
    assert_eq!(version(both, HttpMode::Http1Only).await, Version::HTTP_11);
}

#[tokio::test]
async fn negotiation_against_a_server_without_http2_is_not_forced() {
    let h1 = tls_server(&[b"http/1.1"]).await;
    assert_eq!(version(h1, HttpMode::Negotiate).await, Version::HTTP_11);
}
