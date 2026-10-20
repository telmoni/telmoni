//! The machine door, in front of the lanes a machine reaches straight on the
//! machine host: the read API under `/v1`, the CLI's under `/cli/v1`, pings
//! under `/ping/v1` and ingest under `/otlp`. Each lane gates its own; the
//! door adds the caller's address, read past the proxies in front, and the
//! limits kept in Redis by source and by credential, so one caller cannot
//! spend the others' room. Ingest is metered here by source alone: its
//! credential is its project, which its handler meters once the key is known.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    http::header,
    middleware::{self, Next},
    response::{IntoResponse, Response},
};

use telmoni_shared::TelmoniError;
use telmoni_telemetry::handler::{ingest, pings};
use telmoni_telemetry::rate_limit::RateLimit;

/// How many a minute one credential may make: an API key on `/v1`, a bearer
/// on `/cli/v1`.
const PER_CREDENTIAL: u32 = 600;
/// How many a minute one address may make on `/v1`, keyed or not.
const V1_PER_SOURCE: u32 = 1_200;
/// How many a minute one address may make on `/cli/v1`: a sign-in's polls, a
/// refresh now and then, and little else.
const CLI_PER_SOURCE: u32 = 120;
/// How many a minute one address may make on `/ping/v1`: a host's crontab
/// lines, each pinging its start and its end. The plan's own numbers, per
/// organization and per key, come with the limiter (roadmap 2.11).
const PING_PER_SOURCE: u32 = 1_200;
/// How many a minute one address may make on `/otlp`, keyed or not: `/v1`'s
/// rate, a hundred processes exporting at the batch cadence behind one
/// address, and the most key lookups an unkeyed flood from one address can
/// cost auth in a minute. The key's own limit is the lane's, per project.
const OTLP_PER_SOURCE: u32 = 1_200;

/// The door, as one replica holds it: the limits, and where they are kept.
pub struct Door {
    /// Telemetry holds the server's Redis, which every limit is kept in.
    telemetry: Arc<telmoni_telemetry::AppState>,
    /// How many `X-Forwarded-For` entries the proxies in front append.
    trusted_proxy_hops: u8,
    v1_source: RateLimit,
    v1_key: RateLimit,
    cli_source: RateLimit,
    cli_bearer: RateLimit,
    ping_source: RateLimit,
    ping_key: RateLimit,
    otlp_source: RateLimit,
}

/// Which lanes a request came in on, since each has limits of its own.
#[derive(Debug, Clone, Copy)]
enum Lane {
    V1,
    Cli,
    Ping,
    Otlp,
}

impl Door {
    /// The door over `telemetry`'s Redis, reading the caller's address
    /// `trusted_proxy_hops` entries back from the end of `X-Forwarded-For`.
    #[must_use]
    pub fn new(telemetry: Arc<telmoni_telemetry::AppState>, trusted_proxy_hops: u8) -> Arc<Self> {
        Arc::new(Self {
            telemetry,
            trusted_proxy_hops,
            v1_source: RateLimit::new("telmoni:door:v1:source", V1_PER_SOURCE),
            v1_key: RateLimit::new("telmoni:door:v1:key", PER_CREDENTIAL),
            cli_source: RateLimit::new("telmoni:door:cli:source", CLI_PER_SOURCE),
            cli_bearer: RateLimit::new("telmoni:door:cli:bearer", PER_CREDENTIAL),
            ping_source: RateLimit::new("telmoni:door:ping:source", PING_PER_SOURCE),
            ping_key: RateLimit::new("telmoni:door:ping:key", PER_CREDENTIAL),
            otlp_source: RateLimit::new("telmoni:door:otlp:source", OTLP_PER_SOURCE),
        })
    }

    /// How many `X-Forwarded-For` entries the door reads past, for the boot
    /// line that says what the server was told about its front.
    #[must_use]
    pub const fn trusted_proxy_hops(&self) -> u8 {
        self.trusted_proxy_hops
    }

    /// The `/v1` lanes behind the door's limits.
    pub fn v1(self: &Arc<Self>, lanes: Router) -> Router {
        lanes.layer(middleware::from_fn_with_state(
            (self.clone(), Lane::V1),
            admit,
        ))
    }

    /// The `/cli/v1` lanes behind the door's limits.
    pub fn cli(self: &Arc<Self>, lanes: Router) -> Router {
        lanes.layer(middleware::from_fn_with_state(
            (self.clone(), Lane::Cli),
            admit,
        ))
    }

    /// The `/ping/v1` lane behind the door's limits: by source, and by the key
    /// in the path, before the lane reads it — a key spelled otherwise than
    /// minted refused here, as no key. The door's refusals there are none a
    /// cache may keep, as none of the lane's answers is
    /// ([`pings::never_stored`]): a cache that takes a percent-encoded letter
    /// for the letter, as RFC 9110 §4.2.3 lets one, would keep the refusal of
    /// a key with one letter encoded as the answer for the key itself.
    pub fn ping(self: &Arc<Self>, lanes: Router) -> Router {
        lanes
            .layer(middleware::from_fn_with_state(
                (self.clone(), Lane::Ping),
                admit,
            ))
            .layer(middleware::map_response(pings::never_stored))
    }

    /// Ingest behind the door's limit by source alone: its credential is the
    /// project its key opens, metered in the lane once the key is known, and
    /// the key is looked up only inside the address's allowance, so an
    /// unkeyed flood from one address costs auth a bounded number of
    /// lookups. A refusal here is answered as the lane answers one, OTLP's
    /// `Status` in the request's encoding with `Retry-After`, so an exporter
    /// retries it as it retries the lane's own.
    pub fn otlp(self: &Arc<Self>, lanes: Router) -> Router {
        // The route alone: an SDK's logs and metrics, sent beside it, cost no lookup.
        lanes.route_layer(middleware::from_fn_with_state(
            (self.clone(), Lane::Otlp),
            admit,
        ))
    }

    /// The source's limit, then the credential's where one is presented: a
    /// flood from one address is refused before its credentials are read,
    /// and a credential spent from many addresses is refused all the same;
    /// ingest presents none, its project being metered in the lane.
    async fn admit(
        &self,
        lane: Lane,
        source: &str,
        credential: Option<&str>,
    ) -> Result<(), TelmoniError> {
        let (by_source, by_credential) = match lane {
            Lane::V1 => (&self.v1_source, Some(&self.v1_key)),
            Lane::Cli => (&self.cli_source, Some(&self.cli_bearer)),
            Lane::Ping => (&self.ping_source, Some(&self.ping_key)),
            Lane::Otlp => (&self.otlp_source, None),
        };
        by_source.admit(&self.telemetry.redis, source).await?;
        if let (Some(by_credential), Some(credential)) = (by_credential, credential) {
            by_credential
                .admit(&self.telemetry.redis, credential)
                .await?;
        }
        Ok(())
    }
}

/// The request's address and credential are read before anything waits:
/// the body is not `Sync`, so a request held across an await would keep the
/// middleware's future off the runtime's threads.
async fn admit(
    State((door, lane)): State<(Arc<Door>, Lane)>,
    request: Request,
    next: Next,
) -> Response {
    let source = client_address(&request, door.trusted_proxy_hops);
    let credential = match lane {
        Lane::V1 | Lane::Cli => bearer_token(&request).map(RateLimit::digest),
        Lane::Ping => match ping_key_of(request.uri().path()) {
            Ok(key) => key.map(RateLimit::digest),
            Err(refused) => return refused.into_response(),
        },
        Lane::Otlp => None,
    };
    match door.admit(lane, &source, credential.as_deref()).await {
        Ok(()) => next.run(request).await,
        // Ingest's refusals are OTLP's shape, the door's as the lane's own,
        // sent once the body is read out.
        Err(refused) => match lane {
            Lane::Otlp => ingest::refuse_unread(refused, request).await,
            Lane::V1 | Lane::Cli | Lane::Ping => refused.into_response(),
        },
    }
}

/// ⚠ **A ping's key as it was sent, spelled as minted, or the lane's
/// refusal.** The lane reads its key from the path as axum hands the
/// segments over, percent-decoded, so `telmoniping_%41…` and
/// `telmoniping_A…` are one key to it; to a bucket keyed by the segment as
/// it came they would be two, and every character of the key, encoded or
/// not, a fresh one. So the door takes a key spelled as minted — the prefix
/// and its letters and digits, by the lane's own test — and refuses any
/// other spelling in the key's place as the lane refuses a key it does not
/// hold, before anything is counted, since the answer costs nothing;
/// nothing is decoded, as an encoding of a key is not the key. The key's
/// place is the first segment of the path as the door sees it, the lane's
/// prefix stripped by the nesting; nothing there presents no credential,
/// and the lane's `404` is what it gets. Keyed by its digest, as a bearer
/// is, so Redis holds no key.
fn ping_key_of(path: &str) -> Result<Option<&str>, TelmoniError> {
    let Some(key) = path
        .strip_prefix('/')
        .and_then(|rest| rest.split('/').next())
        .filter(|key| !key.is_empty())
    else {
        return Ok(None);
    };
    if pings::well_formed(key) {
        Ok(Some(key))
    } else {
        Err(pings::unknown_key())
    }
}

/// ⚠ **The token, as the gates read it, and never the header's spelling.**
/// The lanes take the scheme in any case and the token trimmed, so a bucket
/// keyed by the raw header would hand `bEaReR` or one more space a fresh
/// one per request. Anything that is no bearer is no credential here: the
/// gate refuses it, and the source's limit has already counted it.
fn bearer_token(request: &Request) -> Option<&str> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_at_checked(7)?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer ") && !token.is_empty()).then_some(token)
}

/// ⚠ **`X-Forwarded-For`, counted back from the end, and nothing a caller
/// can write.** Each proxy in front appends the address it saw the request
/// come from, so the entry `hops` back from the end is the first one no
/// caller chose: Google's load balancer appends the client's address and
/// then its own (two), a reverse proxy appends the client's (one). Read from
/// the front, or with any header another vendor's edge would set, a caller
/// picks a fresh bucket per request and walks past every per-address
/// ceiling. A line is read whatever bytes a caller put in it, since the
/// proxies append to the caller's own line and a byte past ASCII must not
/// hide the entries after it. Fewer entries than proxies is a request that
/// came in beside them, every entry its own word, so the socket's peer is
/// the address where it is known. With nothing in front (zero) the peer is
/// the address too, and the last entry the caller's word where no peer is
/// known, as the console reads it. An IPv6 address is keyed by its /64, the
/// block one subscriber holds, or one host would walk its own block.
fn client_address(request: &Request, hops: u8) -> String {
    let forwarded: Vec<String> = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .flat_map(|v| {
            v.split(',')
                .map(|part| part.trim().to_owned())
                .collect::<Vec<_>>()
        })
        .filter(|part| !part.is_empty())
        .collect();
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(peer)| peer.ip());
    let beside = hops == 0 || forwarded.len() < usize::from(hops);
    if beside && let Some(peer) = peer {
        return bucket_of(&peer.to_string());
    }
    let index = if hops == 0 {
        forwarded.len().checked_sub(1)
    } else {
        Some(forwarded.len().saturating_sub(usize::from(hops)))
    };
    index
        .and_then(|index| forwarded.get(index))
        .map_or_else(|| "unknown".to_owned(), |address| bucket_of(address))
}

/// An address as its bucket: an IPv4 one whole, written as IPv4 where it
/// came mapped into IPv6; an IPv6 one by its /64; anything that does not
/// parse as it came, since the caller's word is all there is.
fn bucket_of(address: &str) -> String {
    match address.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let [a, b, c, d, ..] = v6.segments();
                format!("{}/64", std::net::Ipv6Addr::new(a, b, c, d, 0, 0, 0, 0))
            }
        },
        Ok(ip) => ip.to_string(),
        Err(_) => address.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::StatusCode;

    fn request(forwarded: &[&str], peer: Option<&str>) -> Request {
        let mut builder = Request::builder().uri("/v1/organization");
        for value in forwarded {
            builder = builder.header("x-forwarded-for", *value);
        }
        let mut request = builder.body(Body::empty()).expect("a request");
        if let Some(peer) = peer {
            let peer: SocketAddr = peer.parse().expect("an address");
            request.extensions_mut().insert(ConnectInfo(peer));
        }
        request
    }

    #[test]
    fn the_address_is_the_first_entry_no_caller_wrote() {
        let behind_google = request(&["203.0.113.9, 198.51.100.7"], Some("10.0.0.5:4"));
        assert_eq!(client_address(&behind_google, 2), "203.0.113.9");
        assert_eq!(client_address(&behind_google, 1), "198.51.100.7");

        let forged = request(&["1.1.1.1, 203.0.113.9, 198.51.100.7"], None);
        assert_eq!(
            client_address(&forged, 2),
            "203.0.113.9",
            "a caller's own entry is read past"
        );

        let beside_the_proxies = request(&["203.0.113.9"], None);
        assert_eq!(client_address(&beside_the_proxies, 2), "203.0.113.9");

        let two_headers = request(&["203.0.113.9", "198.51.100.7"], None);
        assert_eq!(client_address(&two_headers, 2), "203.0.113.9");
    }

    #[test]
    fn with_nothing_in_front_the_peer_is_the_address() {
        let direct = request(&[], Some("192.0.2.4:55000"));
        assert_eq!(client_address(&direct, 0), "192.0.2.4");
        assert_eq!(
            client_address(&direct, 2),
            "192.0.2.4",
            "no entries: the peer"
        );

        let claimed = request(&["203.0.113.9"], None);
        assert_eq!(
            client_address(&claimed, 0),
            "203.0.113.9",
            "no peer known: the caller's word"
        );

        let nothing = request(&[], None);
        assert_eq!(client_address(&nothing, 2), "unknown");
    }

    #[test]
    fn fewer_entries_than_proxies_is_the_peer_where_it_is_known() {
        let beside = request(&["203.0.113.9"], Some("10.0.0.5:4"));
        assert_eq!(
            client_address(&beside, 2),
            "10.0.0.5",
            "an entry no proxy vouched for names nobody"
        );
    }

    #[test]
    fn a_byte_past_ascii_hides_no_entry_after_it() {
        let mut behind_google = request(&[], Some("10.0.0.5:4"));
        behind_google.headers_mut().insert(
            "x-forwarded-for",
            axum::http::HeaderValue::from_bytes(b"\xff\xfe, 203.0.113.9, 198.51.100.7")
                .expect("a header of opaque bytes"),
        );
        assert_eq!(client_address(&behind_google, 2), "203.0.113.9");
    }

    #[test]
    fn an_ipv6_caller_is_one_bucket_for_its_slash_64() {
        let one = request(&["2001:db8:1:2:aaaa::1, 198.51.100.7"], None);
        let two = request(&["2001:db8:1:2:bbbb::9, 198.51.100.7"], None);
        assert_eq!(client_address(&one, 2), "2001:db8:1:2::/64");
        assert_eq!(client_address(&one, 2), client_address(&two, 2));
        let mapped = request(&["::ffff:203.0.113.9, 198.51.100.7"], None);
        assert_eq!(client_address(&mapped, 2), "203.0.113.9");
    }

    fn with_authorization(value: &str) -> Request {
        Request::builder()
            .uri("/v1/organization")
            .header(header::AUTHORIZATION, value)
            .body(Body::empty())
            .expect("a request")
    }

    #[test]
    fn a_credential_is_its_token_however_the_header_spells_it() {
        let plain = with_authorization("Bearer telmoni_abc");
        assert_eq!(bearer_token(&plain), Some("telmoni_abc"));
        for spelling in [
            "bEaReR telmoni_abc",
            "BEARER   telmoni_abc ",
            "bearer telmoni_abc\t",
        ] {
            assert_eq!(
                bearer_token(&with_authorization(spelling)),
                Some("telmoni_abc"),
                "{spelling:?}"
            );
        }
        for not_a_bearer in ["Basic dXNlcjpwYXNz", "Bearer ", "Bearer", "telmoni_abc"] {
            assert_eq!(
                bearer_token(&with_authorization(not_a_bearer)),
                None,
                "{not_a_bearer:?}"
            );
        }
    }

    /// The key with its last character percent-encoded, and with the first
    /// of its prefix: one key to the lane either way, and none to the door.
    fn spelled_otherwise(key: &str) -> [String; 2] {
        let last = key.bytes().next_back().expect("a key has a last byte");
        let head = key
            .strip_suffix(char::from(last))
            .expect("the key ends in its last byte");
        let tail = key.strip_prefix('t').expect("a key starts with its prefix");
        [format!("{head}%{last:02X}"), format!("%74{tail}")]
    }

    /// A ping's credential is the key in its path as the door sees it, the
    /// lane's prefix stripped, spelled as minted; nothing in the key's
    /// place, or a doubled slash before it, is no credential; and anything
    /// else there — the key with a character percent-encoded, in its prefix
    /// or after it, a word, the lane's own name, a key a character short or
    /// long, one with a byte past ASCII or a character beside it — is
    /// refused as the lane refuses a key it does not hold.
    #[test]
    fn a_pings_credential_is_the_key_in_its_path_spelled_as_minted() {
        let key = telmoni_shared::types::mint_ping_key();
        for path in [
            format!("/{key}/nightly"),
            format!("/{key}/nightly/start"),
            format!("/{key}/"),
            format!("/{key}"),
        ] {
            assert_eq!(
                ping_key_of(&path).ok(),
                Some(Some(key.as_str())),
                "{path:?}"
            );
        }
        let doubled = format!("//{key}/nightly");
        for path in ["/", "", "//", doubled.as_str()] {
            assert_eq!(ping_key_of(path).ok(), Some(None), "{path:?}");
        }
        let short = key
            .strip_suffix(|_: char| true)
            .expect("a key has a last character");
        let shorter = short
            .strip_suffix(|_: char| true)
            .expect("a key has a last character");
        let mut not_keys = spelled_otherwise(&key).to_vec();
        not_keys.extend([
            "notakey".to_owned(),
            "ping".to_owned(),
            "telmoniping_".to_owned(),
            short.to_owned(),
            format!("{key}x"),
            format!("{shorter}é"),
            format!("{key};x"),
            format!("{key}%20"),
        ]);
        for not_a_key in not_keys {
            let path = format!("/{not_a_key}/nightly");
            let refused = ping_key_of(&path).expect_err(&path);
            assert_eq!(
                refused.into_response().status(),
                StatusCode::NOT_FOUND,
                "{path:?}"
            );
        }
    }

    /// What the lane reads: axum hands a path's segments over
    /// percent-decoded, so the key with a character encoded, in its prefix
    /// or after it, is the minted key to the lane — which is why the door
    /// takes a key as minted or not at all.
    #[tokio::test]
    async fn the_lane_reads_its_key_decoded() {
        use axum::{Router, extract::Path, routing::get};
        use tower::ServiceExt as _;

        let lane = Router::new().route(
            "/{key}/{slug}",
            get(|Path((key, _)): Path<(String, String)>| async move { key }),
        );
        let key = telmoni_shared::types::mint_ping_key();
        for spelling in spelled_otherwise(&key) {
            let request = Request::get(format!("/{spelling}/nightly"))
                .body(Body::empty())
                .expect("a request");
            let response = lane
                .clone()
                .oneshot(request)
                .await
                .expect("the lane answers");
            let read = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .expect("a body");
            assert_eq!(read, key.as_bytes(), "{spelling:?}");
        }
    }
}
