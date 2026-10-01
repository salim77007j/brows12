//! hyper + rustls HTTP client: HTTP/1.1 and HTTP/2, redirects, cookies.

use crate::{CookieStore, NetError};
use bytes::Bytes;
use http_body_util::{BodyExt, Empty};

use hyper::header::SET_COOKIE;
use hyper::Request;
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;

pub type HyperClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Empty<Bytes>>;

/// Tunable knobs for the HTTP stack.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub user_agent: String,
    pub max_redirects: usize,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    /// Advertised Accept-Language.
    pub accept_language: String,
    /// Disallow insecure TLS versions below 1.2 (always enforced).
    pub allow_http_fallback: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            user_agent: format!("Brows12/{}", env!("CARGO_PKG_VERSION")),
            max_redirects: 10,
            connect_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(30),
            accept_language: "en-US,en;q=0.9".into(),
            allow_http_fallback: false,
        }
    }
}

/// One outbound request.
#[derive(Debug, Clone)]
pub struct NetRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// CHIPS partition of the embedding document.
    pub top_level_site: String,
}

impl NetRequest {
    pub fn get(url: impl Into<String>, top_level_site: impl Into<String>) -> Self {
        NetRequest {
            url: url.into(),
            method: "GET".into(),
            headers: Vec::new(),
            body: None,
            top_level_site: top_level_site.into(),
        }
    }
}

/// Completed response (body fully buffered — pages are bounded by memory
/// budgeting upstream).
#[derive(Debug, Clone)]
pub struct NetResponse {
    pub url: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub protocol: String,
}

impl NetResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn set_cookie_headers(&self) -> Vec<String> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(SET_COOKIE.as_str()))
            .map(|(_, v)| v.clone())
            .collect()
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// The engine's HTTP client.
pub struct HttpClient {
    inner: Arc<HyperClient>,
    pub config: ClientConfig,
    pub cookies: Option<Arc<dyn CookieStore>>,
}

impl HttpClient {
    /// Build a client with the given config; TLS 1.2/1.3 via rustls+ring,
    /// Mozilla root store, HTTP/2 when the server supports it.
    pub fn new(config: ClientConfig, cookies: Option<Arc<dyn CookieStore>>) -> Self {
        let https = HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        let inner = Client::builder(TokioExecutor::new()).build(https);
        Self { inner: Arc::new(inner), config, cookies }
    }

    /// Execute a request, following redirects and applying cookies.
    pub async fn send(&self, mut req: NetRequest) -> Result<NetResponse, NetError> {
        let mut redirects = 0usize;
        loop {
            let url = url::Url::parse(&req.url).map_err(|_| NetError::InvalidUrl(req.url.clone()))?;
            let is_third_party = !same_site(&url, &req.top_level_site);

            let mut builder = Request::builder()
                .method(req.method.as_str())
                .uri(url.as_str())
                .header("user-agent", self.config.user_agent.clone())
                .header("accept-language", self.config.accept_language.clone())
                .header("accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");

            if let Some(cookies) = &self.cookies {
                if let Some(header) = cookies.header_for(&url, &req.top_level_site, is_third_party) {
                    builder = builder.header("cookie", header);
                }
            }
            for (k, v) in &req.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }

            let request = builder
                .body(Empty::<Bytes>::new())
                .map_err(|e| NetError::Http(e.to_string()))?;

            let response = tokio::time::timeout(
                self.config.response_timeout,
                self.inner.request(request),
            )
            .await
            .map_err(|_| NetError::Http("response timeout".into()))?
            .map_err(|e| NetError::Http(e.to_string()))?;

            let status = response.status().as_u16();
            let protocol = format!("{:?}", response.version());
            let headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();

            let body = response
                .into_body()
                .collect()
                .await
                .map_err(|e| NetError::Http(e.to_string()))?
                .to_bytes()
                .to_vec();

            // Record Set-Cookie before deciding on redirects.
            if let Some(cookies) = &self.cookies {
                let set_cookies: Vec<String> = headers
                    .iter()
                    .filter(|(k, _)| k.eq_ignore_ascii_case(SET_COOKIE.as_str()))
                    .map(|(_, v)| v.clone())
                    .collect();
                if !set_cookies.is_empty() {
                    cookies.record(&url, &set_cookies, &req.top_level_site);
                }
            }

            // Redirects (3xx with Location).
            if (300..400).contains(&status) {
                if let Some(location) = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("location"))
                    .map(|(_, v)| v.clone())
                {
                    let next = url
                        .join(&location)
                        .map_err(|_| NetError::InvalidUrl(location.clone()))?;
                    redirects += 1;
                    if redirects > self.config.max_redirects {
                        return Err(NetError::TooManyRedirects);
                    }
                    req.url = next.to_string();
                    req.method = if status == 303 || ((status == 301 || status == 302) && req.method == "POST") {
                        "GET".into()
                    } else {
                        req.method
                    };
                    req.body = None;
                    continue;
                }
            }

            return Ok(NetResponse {
                url: req.url.clone(),
                status,
                headers,
                body,
                protocol,
            });
        }
    }

    /// Preconnect (TCP) to a host — used by the speculative connection pool.
    pub async fn preconnect(&self, host: &str, port: u16) -> std::io::Result<()> {
        let addr = format!("{host}:{port}");
        let _stream = tokio::time::timeout(
            self.config.connect_timeout,
            TcpStream::connect(addr),
        )
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "preconnect timeout"))??;
        Ok(())
    }
}

fn same_site(url: &url::Url, top_level_site: &str) -> bool {
    let request_host = url.host_str().unwrap_or("");
    let request_site = brows12_storage::registrable_domain(request_host);
    let top = brows12_storage::registrable_domain(top_level_site);
    !top.is_empty() && request_site == top
}

/// Convenience: run the provided async closure on the engine's tokio runtime.
pub fn block_on<F: std::future::Future>(rt: &tokio::runtime::Runtime, fut: F) -> F::Output {
    rt.block_on(fut)
}

#[allow(unused_imports)]
use tokio::io::AsyncRead; // keep TokioIo dependency referenced on Windows cfg paths

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let cfg = ClientConfig::default();
        assert!(cfg.user_agent.starts_with("Brows12/"));
        assert_eq!(cfg.max_redirects, 10);
        assert!(!cfg.allow_http_fallback);
    }

    #[test]
    fn same_site_detection() {
        let u = url::Url::parse("https://cdn.example.com/x.js").unwrap();
        assert!(same_site(&u, "example.com"));
        assert!(!same_site(&u, "tracker.io"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetches_local_http_server() {
        // Tiny hand-rolled HTTP/1.1 server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut sock, &mut buf).await;
            let resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 5\r\n\r\nhello";
            tokio::io::AsyncWriteExt::write_all(&mut sock, resp).await.unwrap();
        });

        let client = HttpClient::new(ClientConfig::default(), None);
        let resp = client
            .send(NetRequest::get(format!("http://{addr}/"), ""))
            .await
            .unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello".to_vec());
        assert_eq!(resp.header("content-type"), Some("text/html"));
    }
}
