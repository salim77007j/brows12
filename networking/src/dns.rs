//! RFC 8484 DNS-over-HTTPS client.
//!
//! A deliberately minimal, dependency-free implementation: DNS wire-format
//! queries are constructed and parsed directly (A, AAAA, CNAME with full
//! name decompression). Runs over the engine's existing HTTPS client, so it
//! benefits from the same TLS 1.3 / HTTP/2 transport, connection pooling and
//! zero-copy body handling as page loads.

use crate::NetError;
use base64::Engine as _;
use std::sync::Arc;

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;
const TYPE_CNAME: u16 = 5;
const CLASS_IN: u16 = 1;

/// Public DoH endpoints shipped with the engine.
pub const CLOUDFLARE_DOH: &str = "https://cloudflare-dns.com/dns-query";
pub const GOOGLE_DOH: &str = "https://dns.google/dns-query";

/// Result of a resolution attempt.
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub ipv4: Vec<std::net::Ipv4Addr>,
    pub ipv6: Vec<std::net::Ipv6Addr>,
    /// CNAME chain discovered during resolution (closest-to-name first).
    pub cname_chain: Vec<String>,
}

/// Async transport used to reach the DoH server (decouples from hyper).
pub trait HttpTransport: Send + Sync {
    /// Perform a GET, return the response body on 200.
    fn get(&self, url: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>, NetError>> + Send>>;
}

/// DNS-over-HTTPS resolver.
pub struct DohClient {
    endpoint: String,
    transport: Arc<dyn HttpTransport>,
}

impl DohClient {
    pub fn new(endpoint: &str, transport: Arc<dyn HttpTransport>) -> Self {
        Self { endpoint: endpoint.to_string(), transport }
    }

    /// Resolve A + AAAA records for `name`, collecting the CNAME chain.
    pub async fn resolve(&self, name: &str) -> Result<Resolution, NetError> {
        let name = name.trim_end_matches('.');
        let mut a = self.query_records(name, TYPE_A).await?;
        let aaaa = self.query_records(name, TYPE_AAAA).await?;
        a.ipv6.extend(aaaa.ipv6);
        a.cname_chain.extend(aaaa.cname_chain);
        Ok(a)
    }

    async fn query_records(&self, name: &str, rtype: u16) -> Result<Resolution, NetError> {
        let query = build_query(name, rtype);
        let wire = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(query);
        let url = format!("{}?dns={}", self.endpoint, wire);
        let body = self.transport.get(url).await?;

        parse_response(&body, name, rtype)
            .ok_or(NetError::Dns(format!("malformed DoH response for {name}")))
    }
}

/// Build a minimal DNS query message (one question, RD=1).
pub fn build_query(name: &str, rtype: u16) -> Vec<u8> {
    let mut msg = Vec::with_capacity(17 + name.len());
    msg.extend_from_slice(&[0xAB, 0xCD, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    for label in name.split('.') {
        if label.is_empty() {
            continue;
        }
        msg.push(label.len() as u8);
        msg.extend_from_slice(label.as_bytes());
    }
    msg.push(0);
    msg.extend_from_slice(&rtype.to_be_bytes());
    msg.extend_from_slice(&CLASS_IN.to_be_bytes());
    msg
}

/// Parse a DNS response: addresses + CNAME chain for the question name.
pub fn parse_response(wire: &[u8], expect_name: &str, rtype: u16) -> Option<Resolution> {
    if wire.len() < 12 {
        return None;
    }
    let rcode = wire[3] & 0x0F;
    if rcode != 0 {
        return Some(Resolution::default());
    }
    let qdcount = u16::from_be_bytes([wire[4], wire[5]]);
    let ancount = u16::from_be_bytes([wire[6], wire[7]]);

    let mut pos = 12usize;
    for _ in 0..qdcount {
        skip_name(wire, &mut pos)?;
        pos += 4; // qtype + qclass
    }

    let mut resolution = Resolution::default();
    for _ in 0..ancount {
        let _ = read_name(wire, &mut pos)?; // owner name
        if pos + 10 > wire.len() {
            return None;
        }
        let rt = u16::from_be_bytes([wire[pos], wire[pos + 1]]);
        let rdlength = u16::from_be_bytes([wire[pos + 8], wire[pos + 9]]) as usize;
        pos += 10;
        let rdata_start = pos;
        let rdata_end = pos.checked_add(rdlength)?;
        if rdata_end > wire.len() {
            return None;
        }
        match (rt, rdlength) {
            (TYPE_A, 4) => {
                resolution.ipv4.push(std::net::Ipv4Addr::new(
                    wire[rdata_start],
                    wire[rdata_start + 1],
                    wire[rdata_start + 2],
                    wire[rdata_start + 3],
                ));
            }
            (TYPE_AAAA, 16) => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&wire[rdata_start..rdata_end]);
                resolution.ipv6.push(std::net::Ipv6Addr::from(octets));
            }
            (TYPE_CNAME, _) => {
                let mut p = rdata_start;
                if let Some(target) = read_name(wire, &mut p) {
                    if !resolution.cname_chain.iter().any(|c| c == &target) {
                        resolution.cname_chain.push(target);
                    }
                }
            }
            _ => {}
        }
        pos = rdata_end;
    }
    let _ = (expect_name, rtype);
    Some(resolution)
}

fn skip_name(wire: &[u8], pos: &mut usize) -> Option<()> {
    read_name(wire, pos)?;
    Some(())
}

/// Read (possibly compressed) domain name, advancing `pos`.
fn read_name(wire: &[u8], pos: &mut usize) -> Option<String> {
    let mut labels = Vec::new();
    let mut jumps = 0;
    let mut cursor = *pos;
    let mut followed_pointer = false;
    loop {
        if cursor >= wire.len() {
            return None;
        }
        let len = wire[cursor];
        match len {
            0 => {
                cursor += 1;
                if !followed_pointer {
                    *pos = cursor;
                }
                break;
            }
            0xC0..=0xFF => {
                if cursor + 1 >= wire.len() || jumps > 8 {
                    return None;
                }
                let offset = (((len & 0x3F) as usize) << 8) | wire[cursor + 1] as usize;
                if !followed_pointer {
                    *pos = cursor + 2;
                    followed_pointer = true;
                }
                cursor = offset;
                jumps += 1;
            }
            _ => {
                let start = cursor + 1;
                let end = start + len as usize;
                if end > wire.len() {
                    return None;
                }
                labels.push(String::from_utf8_lossy(&wire[start..end]).to_string());
                cursor = end;
            }
        }
    }
    Some(labels.join("."))
}

/// Resolver wrapper implementing hostname resolution on top of DoH.
pub struct DohResolver {
    pub client: DohClient,
}

impl DohResolver {
    pub async fn resolve_ips(&self, host: &str) -> Result<Resolution, NetError> {
        self.client.resolve(host).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_format() {
        let q = build_query("example.com", TYPE_A);
        assert_eq!(&q[0..2], &[0xAB, 0xCD]); // id
        assert_eq!(q[2], 0x01); // QR=0, Opcode=0, AA=0, TC=0, RD=1
        assert_eq!(q[12], 7); // first label length
        assert_eq!(&q[13..20], b"example");
        let qtype = u16::from_be_bytes([q[q.len() - 4], q[q.len() - 3]]);
        assert_eq!(qtype, TYPE_A);
    }

    #[test]
    fn parses_cname_chain_and_addresses() {
        // Hand-built response: CNAME alias → A record.
        let mut msg = vec![0xAB, 0xCD, 0x81, 0x80, 0, 1, 0, 2, 0, 0, 0, 0];
        // Question: www.foo.com A
        for label in ["www", "foo", "com"] {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        // Answer 1: www.foo.com CNAME edge.foo.net (TTL 300, rdlen = 1+3+1+3+1+... compute)
        let target: Vec<u8> = {
            let mut t = vec![4];
            t.extend_from_slice(b"edge");
            t.push(3);
            t.extend_from_slice(b"foo");
            t.push(3);
            t.extend_from_slice(b"net");
            t.push(0);
            t
        };
        for label in ["www", "foo", "com"] {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&TYPE_CNAME.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&300u32.to_be_bytes());
        msg.extend_from_slice(&(target.len() as u16).to_be_bytes());
        msg.extend_from_slice(&target);
        // Answer 2: edge.foo.net A 93.184.216.34
        let mut owner = vec![4];
        owner.extend_from_slice(b"edge");
        owner.push(3);
        owner.extend_from_slice(b"foo");
        owner.push(3);
        owner.extend_from_slice(b"net");
        owner.push(0);
        msg.extend_from_slice(&owner);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&60u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[93, 184, 216, 34]);

        let res = parse_response(&msg, "www.foo.com", TYPE_A).unwrap();
        assert_eq!(res.ipv4.len(), 1);
        assert_eq!(res.ipv4[0].to_string(), "93.184.216.34");
        assert_eq!(res.cname_chain, vec!["edge.foo.net".to_string()]);
    }

    #[test]
    fn name_compression_is_resolved() {
        // Question + one answer whose name is a pointer to offset 12.
        let mut msg = vec![0, 1, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0];
        for label in ["a", "b"] {
            msg.push(label.len() as u8);
            msg.extend_from_slice(label.as_bytes());
        }
        msg.push(0);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        // Answer with pointer (0xC0, 0x0C) owner.
        msg.extend_from_slice(&[0xC0, 0x0C]);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&60u32.to_be_bytes());
        msg.extend_from_slice(&4u16.to_be_bytes());
        msg.extend_from_slice(&[1, 2, 3, 4]);

        let res = parse_response(&msg, "a.b", TYPE_A).unwrap();
        assert_eq!(res.ipv4.len(), 1);
    }
}
