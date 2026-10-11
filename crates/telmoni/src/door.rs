//! The machine door: what the binary puts in front of the lanes a machine
//! reaches straight on the machine host — the public read API under `/v1`
//! and the CLI's session lanes under `/cli` — because they come in from the
//! internet rather than from the console. Each lane has a gate of its own,
//! an API key, a bearer or a device code; the door adds what every lane
//! shares: the caller's address, read past the proxies in front, and the
//! limits held in Redis by source and by credential, so one caller cannot
//! spend the others' room. Ingest at `/otlp/v1/traces` keeps its own limit,
//! per project, inside its handler.

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
use telmoni_telemetry::rate_limit::RateLimit;

/// How many a minute one credential may make: an API key on `/v1`, a bearer
/// on `/cli`.
const PER_CREDENTIAL: u32 = 600;
/// How many a minute one address may make on `/v1`, keyed or not.
const V1_PER_SOURCE: u32 = 1_200;
/// How many a minute one address may make on `/cli`: a sign-in's polls, a
/// refresh now and then, and little else.
const CLI_PER_SOURCE: u32 = 120;

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
}

/// Which lanes a request came in on, since each has limits of its own.
#[derive(Debug, Clone, Copy)]
enum Lane {
    V1,
    Cli,
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

    /// The `/cli` lanes behind the door's limits.
    pub fn cli(self: &Arc<Self>, lanes: Router) -> Router {
        lanes.layer(middleware::from_fn_with_state(
            (self.clone(), Lane::Cli),
            admit,
        ))
    }

    /// The source's limit, then the credential's where one is presented: a
    /// flood from one address is refused before its credentials are read,
    /// and a credential spent from many addresses is refused all the same.
    async fn admit(
        &self,
        lane: Lane,
        source: &str,
        credential: Option<&str>,
    ) -> Result<(), TelmoniError> {
        let (by_source, by_credential) = match lane {
            Lane::V1 => (&self.v1_source, &self.v1_key),
            Lane::Cli => (&self.cli_source, &self.cli_bearer),
        };
        by_source.admit(&self.telemetry.redis, source).await?;
        if let Some(credential) = credential {
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
    let credential = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(RateLimit::digest);
    match door.admit(lane, &source, credential.as_deref()).await {
        Ok(()) => next.run(request).await,
        Err(refused) => refused.into_response(),
    }
}

/// ⚠ **`X-Forwarded-For`, counted back from the end, and nothing a caller
/// can write.** Each proxy in front appends the address it saw the request
/// come from, so the entry `hops` back from the end is the first one no
/// caller chose: Google's load balancer appends the client's address and
/// then its own (two), a reverse proxy appends the client's (one). Read from
/// the front, or with any header another vendor's edge would set, a caller
/// picks a fresh bucket per request and walks past every per-address
/// ceiling. Fewer entries than proxies is a request that came in beside
/// them, and its first entry is all there is. With nothing in front (zero)
/// the socket's own peer is the address, and the last entry the caller's
/// word where it sent a header anyway, as the console reads it.
fn client_address(request: &Request, hops: u8) -> String {
    let forwarded: Vec<&str> = request
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if (hops == 0 || forwarded.is_empty())
        && let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<SocketAddr>>()
    {
        return peer.ip().to_string();
    }
    if let Some(last) = forwarded.last() {
        let index = if hops == 0 {
            forwarded.len() - 1
        } else {
            forwarded.len().saturating_sub(usize::from(hops))
        };
        return forwarded.get(index).copied().unwrap_or(last).to_owned();
    }
    "unknown".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

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
}
