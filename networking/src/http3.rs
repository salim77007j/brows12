//! HTTP/3 over QUIC (feature `http3`): quinn transport + h3 connection.
//!
//! v1 exposes a single-request GET used for hosts advertising `h3` via
//! Alt-Svc; transparent protocol negotiation is on the roadmap
//! (see docs/ROADMAP.md).

use crate::{NetError, NetRequest, NetResponse};
use http::Method;
use rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
use std::sync::Arc;

#[derive(Debug)]
/// NOTE on certificate verification: the engine pins the webpki root store
/// into the QUIC TLS session. The verifier below delegates to rustls'
/// standard WebPkiServerVerifier; the insecure verifier exists ONLY for
/// tests and is never used by the engine.
struct WebPkiVerifier {
    inner: Arc<rustls::client::WebPkiServerVerifier>,
}

impl ServerCertVerifier for WebPkiVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

/// Build a QUIC client endpoint configured for h3.
pub fn build_quinn_endpoint() -> Result<quinn::Endpoint, NetError> {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| NetError::Tls(e.to_string()))?;

    let mut tls = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(WebPkiVerifier { inner: verifier }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"h3".to_vec()];

    let quic_config = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| NetError::Tls(e.to_string()))?;
    let transport = quinn::TransportConfig::default();
    let mut client_cfg = quinn::ClientConfig::new(Arc::new(quic_config));
    client_cfg.transport_config(Arc::new(transport));

    let mut endpoint = quinn::Endpoint::client("0.0.0.0:0".parse().unwrap())
        .map_err(|e| NetError::Http(format!("quinn endpoint: {e}")))?;
    endpoint.set_default_client_config(client_cfg);
    Ok(endpoint)
}

/// Perform a GET over HTTP/3.
pub async fn h3_get(endpoint: &quinn::Endpoint, req: NetRequest) -> Result<NetResponse, NetError> {
    let url = url::Url::parse(&req.url).map_err(|_| NetError::InvalidUrl(req.url.clone()))?;
    let host = url.host_str().ok_or_else(|| NetError::InvalidUrl(req.url.clone()))?.to_string();
    let port = url.port_or_known_default().unwrap_or(443);

    // Resolve once (v1: system resolver; DoH integration on the roadmap).
    let addr = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|e| NetError::Dns(format!("{host}: {e}")))?
        .next()
        .ok_or_else(|| NetError::Dns(host.clone()))?;

    let connection = endpoint
        .connect(addr, host.as_str())
        .map_err(|e| NetError::Http(e.to_string()))?
        .await
        .map_err(|e| NetError::Http(format!("quic handshake: {e}")))?;

    let (mut conn, mut sender) = h3::client::new(h3_quinn::Connection::new(connection))
        .await
        .map_err(|e| NetError::Http(e.to_string()))?;

    // Drive the h3 connection to completion in the background.
    let driver = tokio::spawn(async move {
        std::future::poll_fn(|cx| conn.poll_close(cx)).await;
    });

    let mut builder = http::Request::builder()
        .method(Method::GET)
        .uri(req.url.as_str())
        .header("user-agent", "Brows12/0.1");
    for (k, v) in &req.headers {
        builder = builder.header(k.as_str(), v.as_str());
    }
    let request = builder.body(()).map_err(|e| NetError::Http(e.to_string()))?;

    let mut stream = sender
        .send_request(request)
        .await
        .map_err(|e| NetError::Http(e.to_string()))?;

    let response = stream
        .recv_response()
        .await
        .map_err(|e| NetError::Http(e.to_string()))?;

    let status = response.status().as_u16();
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let mut body = Vec::new();
    use bytes::Buf;
    while let Some(chunk) = stream
        .recv_data()
        .await
        .map_err(|e| NetError::Http(e.to_string()))?
    {
        body.extend_from_slice(chunk.chunk());
    }

    let _ = driver.abort();
    Ok(NetResponse {
        url: req.url.clone(),
        status,
        headers,
        body,
        protocol: "HTTP/3".into(),
    })
}
