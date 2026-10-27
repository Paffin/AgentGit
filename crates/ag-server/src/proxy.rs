//! Who is really calling: the peer address of the connection, or - only when that peer is a configured trusted
//! proxy (`server.trusted_proxies`) - the client address that proxy reports in `Forwarded` (RFC 7239) or
//! `X-Forwarded-For`.
//!
//! A header from anyone else is ignored: a client that connects directly cannot choose the address that appears in
//! the logs or that its failed logins are counted against. Behind a chain of trusted proxies the header is read from
//! the right, skipping the trusted hops, and the first hop that is not trusted is the client (the leftmost entries
//! are whatever the client itself sent).

use std::net::{IpAddr, Ipv6Addr, SocketAddr};

/// More hops than this in a forwarding header are not read (the rightmost ones are used).
const MAX_HOPS: usize = 32;

/// One trusted network: an address and a prefix length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

/// The networks whose forwarding headers are believed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustedProxies(Vec<Cidr>);

/// An IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is the IPv4 address it carries: a dual-stack listener reports
/// IPv4 peers that way.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

fn mask_v6(prefix: u8) -> u128 {
    if prefix == 0 { 0 } else { u128::MAX << (128 - u32::from(prefix)) }
}

impl Cidr {
    fn parse(s: &str) -> Result<Cidr, String> {
        let s = s.trim();
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = ip.parse().map_err(|_| "not an IP address or CIDR range".to_string())?;
        let addr = canonical(addr);
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => {
                p.parse::<u8>().ok().filter(|p| *p <= max).ok_or_else(|| format!("the prefix must be 0..={max}"))?
            }
            None => max,
        };
        Ok(Cidr { addr, prefix })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, canonical(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = if self.prefix == 0 { 0 } else { u32::MAX << (32 - u32::from(self.prefix)) };
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = mask_v6(self.prefix);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

impl TrustedProxies {
    /// Parses `server.trusted_proxies`: addresses or CIDR ranges. An entry that is neither is an error naming it.
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self, String> {
        entries
            .iter()
            .map(|e| Cidr::parse(e.as_ref()).map_err(|why| format!("'{}': {why}", e.as_ref())))
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// Whether `ip` is one of the trusted proxies.
    pub fn trusts(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|c| c.contains(ip))
    }

    /// True when no proxy is trusted (headers are never read).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The address to attribute a request to. `peer` is the connection's remote address. A direct peer is reported
    /// as `ip:port`; a client learned from a trusted proxy's header as `ip` (its port, if any, is the proxy's
    /// business). `forwarded` and `x_forwarded_for` are the raw header values; `Forwarded` wins when both are sent.
    pub fn client(
        &self,
        peer: Option<SocketAddr>,
        forwarded: Option<&str>,
        x_forwarded_for: Option<&str>,
    ) -> Option<String> {
        let peer = peer?;
        if self.is_empty() || !self.trusts(peer.ip()) {
            return Some(peer.to_string());
        }
        let hops = match (forwarded, x_forwarded_for) {
            (Some(f), _) => forwarded_hops(f),
            (None, Some(x)) => xff_hops(x),
            (None, None) => return Some(peer.to_string()),
        };
        match hops {
            // A hop that is not an address (`unknown`, an obfuscated token) cannot be walked past.
            None => Some(peer.to_string()),
            Some(hops) if hops.is_empty() => Some(peer.to_string()),
            Some(hops) => {
                let client = hops.iter().rev().find(|ip| !self.trusts(**ip)).or(hops.first()).copied()?;
                Some(canonical(client).to_string())
            }
        }
    }
}

/// An address as it appears in a forwarding header: `1.2.3.4`, `1.2.3.4:5`, `[::1]`, `[::1]:5` or bare IPv6.
fn parse_hop(s: &str) -> Option<IpAddr> {
    let s = s.trim().trim_matches('"');
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa.ip());
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.split_once(']')).map(|(ip, _)| ip) {
        return inner.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    s.parse().ok()
}

/// `X-Forwarded-For: client, proxy1, proxy2`, oldest hop first.
fn xff_hops(value: &str) -> Option<Vec<IpAddr>> {
    let parts: Vec<&str> = value.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    let start = parts.len().saturating_sub(MAX_HOPS);
    parts[start..].iter().map(|p| parse_hop(p)).collect()
}

/// `Forwarded: for=client;proto=https, for="[2001:db8::1]:4711"`, oldest hop first. Only the `for` parameters are read.
fn forwarded_hops(value: &str) -> Option<Vec<IpAddr>> {
    let mut hops = Vec::new();
    for element in value.split(',') {
        for pair in element.split(';') {
            if let Some((name, v)) = pair.split_once('=')
                && name.trim().eq_ignore_ascii_case("for")
            {
                hops.push(v.trim());
            }
        }
    }
    let start = hops.len().saturating_sub(MAX_HOPS);
    hops[start..].iter().map(|h| parse_hop(h)).collect()
}

/// The host part of a remote address as logged (`ip:port` or `ip`), the key under which a caller is counted.
pub fn host_of(remote: &str) -> String {
    parse_hop(remote).map(|ip| canonical(ip).to_string()).unwrap_or_else(|| remote.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxies(entries: &[&str]) -> TrustedProxies {
        TrustedProxies::parse(entries).unwrap()
    }

    fn sa(s: &str) -> Option<SocketAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn ranges_are_parsed_and_matched() {
        let t = proxies(&["10.0.0.0/8", "192.168.1.7", "fd00::/8", "2001:db8::1/128"]);
        for yes in ["10.1.2.3", "10.255.255.255", "192.168.1.7", "fd12::1", "2001:db8::1", "::ffff:10.0.0.9"] {
            assert!(t.trusts(yes.parse().unwrap()), "{yes}");
        }
        for no in ["11.0.0.1", "192.168.1.8", "fe80::1", "2001:db8::2", "9.255.255.255"] {
            assert!(!t.trusts(no.parse().unwrap()), "{no}");
        }
        assert!(proxies(&["0.0.0.0/0"]).trusts("8.8.8.8".parse().unwrap()));
        for bad in ["10.0.0.0/33", "nonsense", "10.0.0.0/x", "::1/129", ""] {
            assert!(TrustedProxies::parse(&[bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn headers_from_a_peer_that_is_not_trusted_are_ignored() {
        let t = proxies(&["10.0.0.0/8"]);
        let spoofed = Some("1.1.1.1");
        assert_eq!(t.client(sa("203.0.113.9:4711"), None, spoofed).unwrap(), "203.0.113.9:4711");
        assert_eq!(t.client(sa("203.0.113.9:4711"), Some("for=1.1.1.1"), None).unwrap(), "203.0.113.9:4711");
        // With nobody trusted, a header is never read, even from a loopback peer.
        assert_eq!(TrustedProxies::default().client(sa("10.0.0.1:1"), None, spoofed).unwrap(), "10.0.0.1:1");
        assert_eq!(t.client(None, None, spoofed), None);
    }

    #[test]
    fn a_trusted_proxy_names_the_client_and_the_chain_is_walked_from_the_right() {
        let t = proxies(&["10.0.0.0/8"]);
        let peer = sa("10.0.0.5:9999");
        assert_eq!(t.client(peer, None, Some("198.51.100.7")).unwrap(), "198.51.100.7");
        assert_eq!(t.client(peer, None, None).unwrap(), "10.0.0.5:9999", "no header: the proxy itself");
        // A client-supplied leftmost entry is not believed past the first untrusted hop from the right.
        assert_eq!(t.client(peer, None, Some("6.6.6.6, 198.51.100.7, 10.0.0.77")).unwrap(), "198.51.100.7");
        assert_eq!(t.client(peer, None, Some("6.6.6.6, 198.51.100.7:5555")).unwrap(), "198.51.100.7");
        // All hops trusted: the leftmost.
        assert_eq!(t.client(peer, None, Some("10.1.1.1, 10.2.2.2")).unwrap(), "10.1.1.1");
        // IPv6, bracketed with a port, and an IPv4-mapped address.
        assert_eq!(t.client(peer, None, Some("[2001:db8::7]:4711")).unwrap(), "2001:db8::7");
        assert_eq!(t.client(peer, None, Some("::ffff:198.51.100.7")).unwrap(), "198.51.100.7");
        // An entry that is not an address cannot be walked past: the proxy is reported.
        assert_eq!(t.client(peer, None, Some("unknown")).unwrap(), "10.0.0.5:9999");
        assert_eq!(t.client(peer, None, Some("198.51.100.7, not-an-ip")).unwrap(), "10.0.0.5:9999");
    }

    #[test]
    fn the_forwarded_header_is_read_and_wins_over_x_forwarded_for() {
        let t = proxies(&["10.0.0.0/8"]);
        let peer = sa("10.0.0.5:9999");
        let f = r#"for=6.6.6.6;proto=https, For="[2001:db8:cafe::17]:4711";by=10.0.0.5, for=10.0.0.9"#;
        assert_eq!(t.client(peer, Some(f), Some("1.2.3.4")).unwrap(), "2001:db8:cafe::17");
        assert_eq!(t.client(peer, Some("for=198.51.100.7:80"), None).unwrap(), "198.51.100.7");
        assert_eq!(t.client(peer, Some("for=_hidden"), Some("1.2.3.4")).unwrap(), "10.0.0.5:9999");
        assert_eq!(t.client(peer, Some("proto=https;by=10.0.0.5"), None).unwrap(), "10.0.0.5:9999");
    }

    #[test]
    fn the_host_of_a_remote_strips_the_port() {
        assert_eq!(host_of("203.0.113.9:4711"), "203.0.113.9");
        assert_eq!(host_of("203.0.113.9"), "203.0.113.9");
        assert_eq!(host_of("[2001:db8::1]:80"), "2001:db8::1");
        assert_eq!(host_of("2001:db8::1"), "2001:db8::1");
        assert_eq!(host_of("::ffff:203.0.113.9"), "203.0.113.9");
        assert_eq!(host_of("-"), "-");
    }
}
