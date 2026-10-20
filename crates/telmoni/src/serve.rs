//! The accept loop: `axum::serve`'s, but for the timer it never gives hyper,
//! without which a request's head has no deadline and a connection that
//! never finishes one is held for good.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::pin;
use std::time::Duration;

use axum::Router;
use axum::serve::Listener;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower::{Service, ServiceExt};

/// How long a connection may take over a request's head, from its accept or
/// its last answer: past the 600 s Google's load balancer keeps an idle
/// connection to a backend, which a backend must outlast or the next request
/// sent on one it closes fails.
pub(crate) const HEAD_WAIT: Duration = Duration::from_secs(620);

/// Serves `router` on `listener`, each request carrying its peer's address,
/// until `signal`; then takes no connection more and waits for those open to
/// finish what they are answering. HTTP/1 alone: nothing speaks HTTP/2 to
/// the server, and the HTTP/2 sniff holds a silent connection with no timer.
pub(crate) async fn serve(
    mut listener: TcpListener,
    router: Router,
    head_wait: Duration,
    signal: impl Future<Output = ()> + Send + 'static,
) {
    let mut connected = router.into_make_service_with_connect_info::<SocketAddr>();
    let (signal_tx, signal_rx) = watch::channel(());
    tokio::spawn(async move {
        signal.await;
        drop(signal_rx);
    });
    let (close_tx, close_rx) = watch::channel(());

    loop {
        let (io, peer) = tokio::select! {
            accepted = Listener::accept(&mut listener) => accepted,
            () = signal_tx.closed() => break,
        };
        let Ok(service) = ServiceExt::<SocketAddr>::ready(&mut connected).await;
        let Ok(service) = service.call(peer).await;
        let service = TowerToHyperService::new(service);
        let signal_tx = signal_tx.clone();
        let close_rx = close_rx.clone();
        tokio::spawn(async move {
            let mut builder = Builder::new(TokioExecutor::new()).http1_only();
            builder
                .http1()
                .timer(TokioTimer::new())
                .header_read_timeout(head_wait);
            let mut connection = pin!(builder.serve_connection(TokioIo::new(io), service));
            let mut draining = false;
            loop {
                tokio::select! {
                    served = connection.as_mut() => {
                        if let Err(error) = served {
                            tracing::trace!(error = %error, "a connection ended");
                        }
                        break;
                    }
                    () = signal_tx.closed(), if !draining => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
            drop(close_rx);
        });
    }

    drop(close_rx);
    drop(listener);
    close_tx.closed().await;
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use axum::extract::ConnectInfo;
    use axum::routing::get;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;
    use tokio::sync::oneshot;
    use tokio::task::JoinHandle;

    use super::*;

    /// A deadline short enough to see a connection given up within a test.
    const WAIT: Duration = Duration::from_millis(300);

    /// The loop on a port of its own, with its stop and its end.
    async fn started(head_wait: Duration) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router =
            Router::new()
                .route(
                    "/peer",
                    get(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move {
                        peer.ip().to_string()
                    }),
                )
                .route(
                    "/slow",
                    get(|| async {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        "done"
                    }),
                );
        let (stop, stopped) = oneshot::channel::<()>();
        let served = tokio::spawn(serve(listener, router, head_wait, async {
            let _ = stopped.await;
        }));
        (address, stop, served)
    }

    /// How long the server took to close `stream`, if it did within `limit`.
    async fn closed_within(stream: &mut TcpStream, limit: Duration) -> Option<Duration> {
        let from = Instant::now();
        let mut buffer = [0_u8; 1024];
        let closed = async {
            while let Ok(read) = stream.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
            }
        };
        tokio::time::timeout(limit, closed)
            .await
            .ok()
            .map(|()| from.elapsed())
    }

    /// One whole answer, read to its `Content-Length`.
    async fn answer(stream: &mut TcpStream) -> String {
        let mut read = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let text = String::from_utf8_lossy(&read).into_owned();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|n| n.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if body.len() >= length {
                    return text;
                }
            }
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0, "the answer was cut short: {text:?}");
            read.extend_from_slice(&buffer[..n]);
        }
    }

    /// A head never finished is given up at its deadline, where it was held for good.
    #[tokio::test]
    async fn a_head_never_finished_is_closed_at_its_deadline() {
        let (address, _stop, _served) = started(WAIT).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /peer HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        let after = closed_within(&mut stream, Duration::from_secs(10))
            .await
            .expect("still open at ten seconds");
        assert!(
            after >= WAIT / 2,
            "closed after {after:?}, short of its deadline"
        );
    }

    /// A connection idle after its answer is given up at the same deadline.
    #[tokio::test]
    async fn an_idle_connection_is_closed_at_its_deadline() {
        let (address, _stop, _served) = started(WAIT).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /peer HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        assert!(answer(&mut stream).await.starts_with("HTTP/1.1 200"));
        let after = closed_within(&mut stream, Duration::from_secs(10))
            .await
            .expect("still open at ten seconds");
        assert!(
            after >= WAIT / 2,
            "closed after {after:?}, short of its deadline"
        );
    }

    /// A connection that sends nothing is given up at the deadline, no protocol sniff holding it.
    #[tokio::test]
    async fn a_silent_connection_is_closed_at_its_deadline() {
        let (address, _stop, _served) = started(WAIT).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        let after = closed_within(&mut stream, Duration::from_secs(10))
            .await
            .expect("still open at ten seconds");
        assert!(
            after >= WAIT / 2,
            "closed after {after:?}, short of its deadline"
        );
    }

    /// The deadline outlasts the 600 s Google's load balancer keeps an idle connection.
    #[test]
    fn the_deadline_outlasts_the_load_balancers_idle_connections() {
        assert!(HEAD_WAIT > Duration::from_secs(600));
    }

    /// A request carries its peer's address, which the door reads.
    #[tokio::test]
    async fn a_request_carries_its_peers_address() {
        let (address, _stop, _served) = started(WAIT).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /peer HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let answer = answer(&mut stream).await;
        assert!(
            answer.starts_with("HTTP/1.1 200") && answer.ends_with("\r\n\r\n127.0.0.1"),
            "{answer:?}"
        );
    }

    /// The stop lets an answer under way finish, then ends without waiting out an idle connection.
    #[tokio::test]
    async fn a_stop_drains_the_answer_under_way() {
        let (address, stop, served) = started(Duration::from_secs(60)).await;
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop.send(()).unwrap();
        let answer = answer(&mut stream).await;
        assert!(
            answer.starts_with("HTTP/1.1 200") && answer.ends_with("done"),
            "{answer:?}"
        );
        tokio::time::timeout(Duration::from_secs(5), served)
            .await
            .expect("the drain never ended")
            .unwrap();
    }
}
