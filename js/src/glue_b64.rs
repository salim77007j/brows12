//! Base64 helpers for the JS boundary (bytes cross as base64 strings).

/// URL-safe-free standard base64 encode.
pub fn encode(data: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Decode base64, returning None on malformed input.
pub fn decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}
