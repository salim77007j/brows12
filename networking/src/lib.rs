//! # brows12-net
//!
//! Networking stack for the Brows12 engine.
//!
//! * HTTP/1.1 + HTTP/2 over TLS 1.3 via `hyper` + `rustls` (ring provider,
//!   webpki Mozilla roots — no system trust store dependency, no OpenSSL).
//! * RFC 8484 DNS-over-HTTPS with full wire-format query/response handling
//!   including CNAME chain extraction (feeds the CNAME-cloaking guard).
//! * Optional HTTP/3 over QUIC (`quinn` + `h3`) behind the `http3` feature.
//! * Cookie integration through the [`CookieStore`] trait (implemented by
//!   `brows12-storage`), keeping this crate storage-agnostic.

pub mod client;
pub mod dns;

#[cfg(feature = "http3")]
pub mod http3;

pub use client::{ClientConfig, HttpClient, NetRequest, NetResponse};
pub use dns::{DohClient, DohResolver};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("HTTP client error: {0}")]
    Http(String),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("DNS resolution failed for {0}")]
    Dns(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("request cancelled by policy: {0}")]
    Blocked(String),
}

/// Cookie source hooked into every request (implemented by the cookie jar).
pub trait CookieStore: Send + Sync {
    /// Cookie header to attach for this request URL (may be None).
    fn header_for(&self, url: &url::Url, top_level_site: &str, is_third_party: bool) -> Option<String>;
    /// Record Set-Cookie headers from a response.
    fn record(
        &self,
        url: &url::Url,
        set_cookies: &[String],
        top_level_site: &str,
    );
}
