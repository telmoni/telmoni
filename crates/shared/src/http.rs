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

#[cfg(test)]
mod tests {
    use std::future::IntoFuture as _;

    use axum::Router;
    use axum::response::Redirect;
    use axum::routing::get;

    use super::*;

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
