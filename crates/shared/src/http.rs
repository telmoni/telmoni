//! Outbound HTTP clients. Every one that leaves with a credential, or with a
//! signed body, refuses redirects: following one would hand what it carries
//! to whatever host the answer names.

use std::time::Duration;

/// A client that is named and never redirected.
pub fn builder(user_agent: &str) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(user_agent)
}

/// [`builder`], bounded by `timeout` for the whole exchange.
pub fn client(user_agent: &str, timeout: Duration) -> reqwest::Result<reqwest::Client> {
    builder(user_agent).timeout(timeout).build()
}

/// The far end's `Retry-After`, as digits or a date in RFC 9110's three
/// formats, held to a day so it overflows no date; a past date is no wait.
#[must_use]
pub fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    const DAY: u64 = 86_400;
    let raw = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let secs = if !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()) {
        raw.parse::<u64>().unwrap_or(u64::MAX)
    } else {
        // IMF-fixdate, then the obsolete RFC 850 and asctime forms, each GMT.
        let when = chrono::DateTime::parse_from_rfc2822(raw)
            .map(|at| at.with_timezone(&chrono::Utc))
            .or_else(|_| {
                chrono::NaiveDateTime::parse_from_str(raw, "%A, %d-%b-%y %H:%M:%S GMT")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%a %b %e %H:%M:%S %Y"))
                    .map(|at| at.and_utc())
            })
            .ok()?;
        (when - chrono::Utc::now())
            .num_seconds()
            .max(0)
            .unsigned_abs()
    };
    Some(Duration::from_secs(secs.min(DAY)))
}

#[cfg(test)]
mod tests {
    use std::future::IntoFuture as _;

    use axum::Router;
    use axum::response::Redirect;
    use axum::routing::get;

    use super::*;

    fn headers(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
        h
    }

    #[test]
    fn retry_after_reads_both_forms_the_rfc_allows() {
        assert_eq!(retry_after(&headers("30")), Some(Duration::from_secs(30)));
        assert_eq!(retry_after(&headers("  7 ")), Some(Duration::from_secs(7)));
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(120)).to_rfc2822();
        let parsed = retry_after(&headers(&soon)).expect("an http-date parses");
        assert!((100..=120).contains(&parsed.as_secs()), "got {parsed:?}");
    }

    #[test]
    fn an_unreadable_retry_after_is_no_instruction_rather_than_no_wait() {
        assert_eq!(retry_after(&reqwest::header::HeaderMap::new()), None);
        assert_eq!(retry_after(&headers("later")), None);
        assert_eq!(retry_after(&headers("")), None);
        assert_eq!(
            retry_after(&headers("-30")),
            None,
            "delay-seconds are digits"
        );
        assert_eq!(retry_after(&headers("+30")), None);
        let past = |format: &str| {
            (chrono::Utc::now() - chrono::Duration::seconds(60))
                .format(format)
                .to_string()
        };
        for older in [
            past("%A, %d-%b-%y %H:%M:%S GMT"),
            past("%a %b %e %H:%M:%S %Y"),
        ] {
            assert_eq!(
                retry_after(&headers(&older)),
                Some(Duration::ZERO),
                "{older}"
            );
        }
        let soon = (chrono::Utc::now() + chrono::Duration::seconds(120))
            .format("%a %b %e %H:%M:%S %Y")
            .to_string();
        let parsed = retry_after(&headers(&soon)).expect("an asctime date parses");
        assert!(
            (100..=120).contains(&parsed.as_secs()),
            "{soon}: {parsed:?}"
        );
        assert_eq!(
            retry_after(&headers("10000000000000")),
            Some(Duration::from_secs(86_400)),
            "a day at most, which no date overflows"
        );
        let past = (chrono::Utc::now() - chrono::Duration::seconds(60)).to_rfc2822();
        assert_eq!(retry_after(&headers(&past)), Some(Duration::ZERO));
    }

    // The answer is the redirect itself, never what it points at.
    #[tokio::test]
    async fn a_client_hands_back_a_redirect_rather_than_following_it() {
        let app = Router::new()
            .route("/", get(|| async { Redirect::temporary("/elsewhere") }))
            .route("/elsewhere", get(|| async { "followed" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(axum::serve(listener, app).into_future());
        let answer = client("test", Duration::from_secs(5))
            .expect("a client")
            .get(&base)
            .send()
            .await
            .expect("an answer");
        assert_eq!(answer.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            answer
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok()),
            Some("/elsewhere")
        );
    }
}
