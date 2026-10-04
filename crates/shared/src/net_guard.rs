//! The outbound URL guard — the one SSRF discipline for every lane that
//! dials a host somebody else chose.
//!
//! A URL check that can be pointed at `169.254.169.254` is a metadata-credential
//! exfiltration service with a friendly name. The network offers no help:
//! every pod's outbound traffic leaves through an unrestricted NAT, and the
//! cluster's NetworkPolicies hold only what may connect IN. So the guard is
//! code or it is nothing, and **any** lane that dials a URL it did not
//! configure reuses it.
//!
//! Three parts, all needed:
//! - **Registration**: [`host_is_dialable`] on the host before a row exists.
//! - **Dial**: the same check on the URL as it leaves the vault, because hyper
//!   hands an IP-literal host straight to the socket without a resolver.
//! - **Connect**: [`GuardedResolver`], so a NAME that resolves — or later
//!   re-resolves (DNS rebinding) — to a private address is never dialled.
//!
//! A sibling service is out of reach only because its addresses are private: a
//! cluster name (`*.cluster.local`) is refused by name, and every pod and
//! Service address is private space the resolver drops — which holds only
//! while the cluster's pod and Service ranges are private; a cluster given
//! publicly routable ranges loses this half of the guard. A forged POST that
//! got there would still carry no service secret.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Why a URL's host was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostRejection {
    /// The string is not a URL at all.
    Malformed,
    /// The URL carries no host at all.
    Missing,
    /// `localhost`, or a name under a suffix that never leaves the machine or
    /// the cloud: `.localhost`, `.local`, `.internal`.
    Local,
    /// An IP literal that [`is_global_ip`] refuses.
    NotGlobal,
}

impl HostRejection {
    /// The predicate, for a sentence whose subject the caller supplies:
    /// "the endpoint {reason}".
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Malformed => "is not a URL",
            Self::Missing => "names no host",
            Self::Local => "names a local address",
            Self::NotGlobal => "points at a private or local address",
        }
    }
}

impl std::fmt::Display for HostRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

/// The client that leaves the platform.
///
/// Both enforcement points in one type: the resolver for every NAME, and
/// [`host_is_dialable`] on every URL for the literals the resolver never sees.
/// Redirects are never followed — following one would hand the signed body or
/// the credential-bearing URL to whatever host it names.
#[derive(Clone, Debug)]
pub struct Egress {
    client: reqwest::Client,
    guarded: bool,
}

impl Egress {
    /// The guarded client: the resolver, the literal check, no redirects.
    pub fn guarded(timeout: Duration, user_agent: &str) -> reqwest::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(user_agent)
            .dns_resolver(Arc::new(GuardedResolver))
            .build()?;
        Ok(Self {
            client,
            guarded: true,
        })
    }

    /// A client with neither half of the guard, for a test whose far end is
    /// a mock on loopback. A deployed tier builds only the guarded one.
    #[must_use]
    pub fn unguarded(client: reqwest::Client) -> Self {
        Self {
            client,
            guarded: false,
        }
    }

    /// Whether this client carries the guard — `false` only for a test's.
    #[must_use]
    pub const fn is_guarded(&self) -> bool {
        self.guarded
    }

    /// A POST to `url`, refused before any socket is opened if the guard
    /// says so.
    pub fn post(&self, url: &str) -> Result<reqwest::RequestBuilder, HostRejection> {
        self.request(reqwest::Method::POST, url)
    }

    /// A DELETE to `url`, on the same terms as [`Egress::post`].
    pub fn delete(&self, url: &str) -> Result<reqwest::RequestBuilder, HostRejection> {
        self.request(reqwest::Method::DELETE, url)
    }

    fn request(
        &self,
        method: reqwest::Method,
        url: &str,
    ) -> Result<reqwest::RequestBuilder, HostRejection> {
        let parsed = Url::parse(url).map_err(|_| HostRejection::Malformed)?;
        if self.guarded {
            host_is_dialable(&parsed)?;
        }
        Ok(self.client.request(method, parsed))
    }
}

/// The registration-time and dial-time half of the guard: may we dial this
/// URL's HOST? Scheme policy stays with the caller. A name that RESOLVES to a
/// private address is [`GuardedResolver`]'s half, not this one's.
pub fn host_is_dialable(url: &Url) -> Result<(), HostRejection> {
    let Some(host) = url.host_str() else {
        return Err(HostRejection::Missing);
    };
    // The URL parser has already canonicalised `127.1` and `0x7f000001` to
    // dotted quads, so no v4 spelling trick survives to here.
    let literal = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return if is_global_ip(ip) {
            Ok(())
        } else {
            Err(HostRejection::NotGlobal)
        };
    }
    let name = host.trim_end_matches('.').to_ascii_lowercase();
    if name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".local")
        || name.ends_with(".internal")
    {
        return Err(HostRejection::Local);
    }
    Ok(())
}

/// Is an IP globally routable — NOT loopback, private, link-local (which holds
/// the 169.254.169.254 metadata address), unspecified or broadcast? The one
/// classifier both the URL check and the resolver enforce, so they can't drift.
#[must_use]
pub fn is_global_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_global_v4(ip),
        // A v4 address inside a v6 spelling IS the v4 address: a dual-stack
        // socket delivers `::ffff:169.254.169.254` to the metadata server, and
        // NAT64's `64:ff9b::/96` embeds the same way.
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_global_v4(v4);
            }
            let [s0, s1, s2, s3, s4, s5, s6, s7] = ip.segments();
            if s0 == 0x64 && s1 == 0xff9b && [s2, s3, s4, s5] == [0, 0, 0, 0] {
                let [a, b] = s6.to_be_bytes();
                let [c, d] = s7.to_be_bytes();
                return is_global_v4(Ipv4Addr::new(a, b, c, d));
            }
            // ULA, link-local, site-local, multicast, 6to4, Teredo (its inner v4
            // is obfuscated, so refused whole), the local-use NAT64 prefix
            // (RFC 8215: the operator picks where the v4 sits, so refused
            // whole too), documentation and discard.
            !(ip.is_loopback()
                || ip.is_unspecified()
                || (s0 == 0x64 && s1 == 0xff9b && s2 == 1)
                || (s0 & 0xfe00) == 0xfc00
                || (s0 & 0xffc0) == 0xfe80
                || (s0 & 0xffc0) == 0xfec0
                || (s0 & 0xff00) == 0xff00
                || s0 == 0x2002
                || (s0 == 0x2001 && s1 == 0)
                || (s0 == 0x2001 && s1 == 0x0db8)
                || (s0 == 0x100 && [s1, s2, s3] == [0, 0, 0]))
        }
    }
}

fn is_global_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        // "This network", carrier NAT (a cloud's internal ranges), IETF
        // protocol space, benchmarking, reserved: none is a customer endpoint.
        || a == 0
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (b & 0xfe) == 18)
        || a >= 240)
}

/// A reqwest DNS resolver that drops every address that isn't
/// [`is_global_ip`]. reqwest connects with exactly what this returns, so a name
/// that resolves or re-resolves to a private address can never be dialled.
///
/// ⚠ For the client that leaves the platform ONLY: a client carrying this
/// cannot reach the metadata server, which is where the KMS bearer comes from.
#[derive(Debug, Default, Clone)]
pub struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let resolved = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let addrs = keep_global(resolved);
            if addrs.is_empty() {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                    "refusing to connect to {host}: resolves only to non-global addresses"
                )));
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Keep only the globally-routable addresses from a resolver result, split out
/// so it is testable without a DNS lookup.
fn keep_global(addrs: impl Iterator<Item = SocketAddr>) -> Vec<SocketAddr> {
    addrs.filter(|a| is_global_ip(a.ip())).collect()
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[test]
    fn resolver_drops_non_global_addresses() {
        let public = SocketAddr::from((Ipv4Addr::new(203, 0, 113, 9), 443));
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 443));
        let metadata = SocketAddr::from((Ipv4Addr::new(169, 254, 169, 254), 443));
        let v6_ula = SocketAddr::from((Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1), 443));

        assert_eq!(
            keep_global([public, loopback, metadata, v6_ula].into_iter()),
            vec![public],
        );
        assert!(keep_global([loopback, metadata, v6_ula].into_iter()).is_empty());
    }

    /// The v6 spellings of a v4 address reach the v4 address: an IP literal
    /// skips the resolver entirely, so this classifier is the whole guard.
    #[test]
    fn a_v4_address_inside_a_v6_spelling_is_classified_as_the_v4_address() {
        let metadata = Ipv4Addr::new(169, 254, 169, 254);
        assert!(!is_global_ip(IpAddr::V6(metadata.to_ipv6_mapped())));
        assert!(!is_global_ip(IpAddr::V6(
            Ipv4Addr::LOCALHOST.to_ipv6_mapped()
        )));
        assert!(!is_global_ip(IpAddr::V6(
            Ipv4Addr::new(10, 0, 0, 9).to_ipv6_mapped()
        )));
        assert!(!is_global_ip(IpAddr::V6(Ipv6Addr::new(
            0x64, 0xff9b, 0, 0, 0, 0, 0xa9fe, 0xa9fe
        ))));
        assert!(is_global_ip(IpAddr::V6(
            Ipv4Addr::new(203, 0, 113, 9).to_ipv6_mapped()
        )));
        assert!(is_global_ip(IpAddr::V6(Ipv6Addr::new(
            0x2606, 0x4700, 0, 0, 0, 0, 0, 0x1111
        ))));
    }

    #[test]
    fn the_reserved_v4_ranges_are_not_global() {
        for ip in [
            Ipv4Addr::new(0, 1, 2, 3),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(100, 127, 255, 254),
            Ipv4Addr::new(192, 0, 0, 8),
            Ipv4Addr::new(198, 18, 0, 1),
            Ipv4Addr::new(198, 19, 255, 255),
            Ipv4Addr::new(224, 0, 0, 1),
            Ipv4Addr::new(240, 0, 0, 1),
        ] {
            assert!(!is_global_ip(IpAddr::V4(ip)), "{ip} must be refused");
        }
        for ip in [
            Ipv4Addr::new(100, 63, 255, 255),
            Ipv4Addr::new(100, 128, 0, 1),
            Ipv4Addr::new(198, 17, 0, 1),
            Ipv4Addr::new(198, 20, 0, 1),
            Ipv4Addr::new(8, 8, 8, 8),
        ] {
            assert!(is_global_ip(IpAddr::V4(ip)), "{ip} must be allowed");
        }
    }

    #[test]
    fn the_reserved_v6_ranges_are_not_global() {
        for ip in [
            Ipv6Addr::LOCALHOST,
            Ipv6Addr::UNSPECIFIED,
            Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0x2002, 0xa9fe, 0xa9fe, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0x64, 0xff9b, 1, 0, 0, 0, 0xa9fe, 0xa9fe),
            Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1),
            Ipv6Addr::new(0x100, 0, 0, 0, 0, 0, 0, 1),
        ] {
            assert!(!is_global_ip(IpAddr::V6(ip)), "{ip} must be refused");
        }
        assert!(is_global_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
        ))));
    }

    /// The name check is the sentence; the resolver would have refused the
    /// address anyway.
    #[test]
    fn local_names_and_literals_are_refused_and_public_hosts_pass() {
        let ok = |raw: &str| host_is_dialable(&Url::parse(raw).unwrap());
        assert_eq!(ok("https://hooks.example.com/x"), Ok(()));
        assert_eq!(ok("https://203.0.113.9/x"), Ok(()));
        assert_eq!(ok("https://[2606:4700::1111]/x"), Ok(()));
        for (raw, why) in [
            ("https://localhost/x", HostRejection::Local),
            ("https://LOCALHOST./x", HostRejection::Local),
            ("https://app.localhost/x", HostRejection::Local),
            ("https://printer.local/x", HostRejection::Local),
            ("https://metadata.google.internal/x", HostRejection::Local),
            ("https://127.0.0.1/x", HostRejection::NotGlobal),
            ("https://127.1/x", HostRejection::NotGlobal),
            ("https://0x7f000001/x", HostRejection::NotGlobal),
            ("https://169.254.169.254/latest", HostRejection::NotGlobal),
            ("https://10.0.0.9/x", HostRejection::NotGlobal),
            ("https://[::1]/x", HostRejection::NotGlobal),
            (
                "https://[::ffff:169.254.169.254]/x",
                HostRejection::NotGlobal,
            ),
            ("https://[fd00::1]/x", HostRejection::NotGlobal),
        ] {
            assert_eq!(ok(raw), Err(why), "{raw}");
        }
    }
}
