//! An alert's notices as this module routes them: to the connections their
//! route names that are the project's, active, take the kind and are not
//! muted for the route. A landed delivery resets its connection's run under
//! the route, one that fails for good adds to it, and five in a row mute the
//! connection for that route alone, until a member unmutes it. A routed
//! notice answers the connections it was queued to.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use telmoni_notifications::AppState;
use telmoni_notifications::db::{self, NotificationsLane};
use telmoni_notifications::seam::Notifier;
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Notice, Notifications, Route};
use telmoni_shared::test_util::{ServiceRole, service_pool};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId, TelmoniError};

/// A webhook connection of `project`, at `status`, seeded as its owner would
/// leave it, its sealed fields placeholders no send here opens.
async fn connection(
    pool: &PgPool,
    project: &ProjectId,
    organization: &OrganizationId,
    status: &str,
) -> Uuid {
    connection_as(pool, project, organization, status, Uuid::new_v4()).await
}

/// A webhook connection of the project's under the id `id`.
async fn connection_as(
    pool: &PgPool,
    project: &ProjectId,
    organization: &OrganizationId,
    status: &str,
    id: Uuid,
) -> Uuid {
    sqlx::query(
        "INSERT INTO notifications.connections
            (id, project_id, organization_id, provider, external_workspace_id, channel_id,
             channel_name, target_ciphertext, target_nonce, key_version, wrapped_dek, scopes,
             installed_by, status)
         VALUES ($1, $2, $3, 'webhook', 'webhook', $4, 'hooks.example.com', '\\x00', '\\x00', 1,
                 '\\x00', '', 'user_owner', $5)",
    )
    .bind(id)
    .bind(project)
    .bind(organization)
    .bind(id.to_string())
    .bind(status)
    .execute(pool)
    .await
    .expect("a connection");
    id
}

/// The connections `route` sends an incident's opening to, among `targets`,
/// each beside its delivery, as the rows hold them and as the queue answers.
async fn routed(
    service: &PgPool,
    organization: &OrganizationId,
    project: &ProjectId,
    route: &str,
    targets: &[Uuid],
) -> Vec<(Uuid, Uuid)> {
    let mut tx = project_scope(service, project).await.expect("a scope");
    let mut queued = db::enqueue_routed(
        &mut tx,
        organization,
        project,
        NotificationKind::IncidentOpened,
        "Slow calls: alert",
        "The p95 duration of model calls over the last hour is 6200 ms.",
        route,
        targets,
        usize::MAX,
    )
    .await
    .expect("queued");
    let ids: Vec<Uuid> = queued.iter().map(|(id, _)| *id).collect();
    let sent: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, connection_id FROM notifications.deliveries WHERE id = ANY($1)
          ORDER BY connection_id",
    )
    .bind(&ids)
    .fetch_all(tx.conn())
    .await
    .expect("the deliveries");
    tx.commit().await.expect("commit");
    queued.sort_by_key(|(_, connection)| *connection);
    assert_eq!(
        queued, sent,
        "the queue answers each delivery with its connection"
    );
    sent
}

/// The deliveries' outcomes counted, as the delivery loop's bookkeeping does:
/// `failed` ones ended for good first.
async fn settled(pool: &PgPool, service: &PgPool, delivered: &[Uuid], failed: &[Uuid]) {
    sqlx::query("UPDATE notifications.deliveries SET status = 'failed' WHERE id = ANY($1)")
        .bind(failed)
        .execute(pool)
        .await
        .expect("failed for good");
    sqlx::query("UPDATE notifications.deliveries SET status = 'delivered' WHERE id = ANY($1)")
        .bind(delivered)
        .execute(pool)
        .await
        .expect("delivered");
    let mut tx = maintenance_scope(service, NotificationsLane)
        .await
        .expect("the lane");
    db::count_routes(&mut tx, delivered, failed)
        .await
        .expect("counted");
    tx.commit().await.expect("commit");
}

/// ⚠ A routed notice reaches only the route's connections that are the
/// project's own and active.
#[sqlx::test]
async fn a_routed_notice_reaches_the_projects_active_connections_it_names(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let named = connection(&pool, &project, &organization, "active").await;
    let unnamed = connection(&pool, &project, &organization, "active").await;
    let errored = connection(&pool, &project, &organization, "errored").await;
    let elsewhere = connection(&pool, &ProjectId::new(), &organization, "active").await;
    let service = service_pool(&pool, ServiceRole::Notifications);

    let sent = routed(
        &service,
        &organization,
        &project,
        "telemetry:rule:a",
        &[named, errored, elsewhere],
    )
    .await;
    assert_eq!(
        sent.iter().map(|(_, c)| *c).collect::<Vec<_>>(),
        [named],
        "neither the unnamed {unnamed}, the errored nor another project's"
    );
    let route: Option<String> =
        sqlx::query_scalar("SELECT route FROM notifications.deliveries WHERE id = $1")
            .bind(sent[0].0)
            .fetch_one(&pool)
            .await
            .expect("the delivery");
    assert_eq!(route.as_deref(), Some("telemetry:rule:a"));
}

/// ⚠ Five failed deliveries in a row mute a connection for that route
/// alone; a landed one resets the run, and an unmute hears it again.
#[sqlx::test]
async fn five_failures_in_a_row_mute_a_connection_for_its_route_alone(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let flaky = connection(&pool, &project, &organization, "active").await;
    let steady = connection(&pool, &project, &organization, "active").await;
    let service = service_pool(&pool, ServiceRole::Notifications);
    let route = "telemetry:rule:a";
    let send = |targets: Vec<Uuid>| {
        let service = service.clone();
        let (organization, project) = (organization.clone(), project.clone());
        async move { routed(&service, &organization, &project, route, &targets).await }
    };

    for _ in 0..4 {
        let sent = send(vec![flaky]).await;
        settled(&pool, &service, &[], &[sent[0].0]).await;
    }
    let landed = send(vec![flaky]).await;
    settled(&pool, &service, &[landed[0].0], &[]).await;
    for _ in 0..4 {
        let sent = send(vec![flaky]).await;
        settled(&pool, &service, &[], &[sent[0].0]).await;
    }
    let mut tx = project_scope(&service, &project).await.expect("a scope");
    assert!(
        db::muted_for(&mut tx, &project, route)
            .await
            .expect("muted")
            .is_empty(),
        "a landed delivery reset the run, so four in a row mute nothing"
    );
    tx.commit().await.expect("commit");

    let fifth = send(vec![flaky]).await;
    settled(&pool, &service, &[], &[fifth[0].0]).await;
    let mut tx = project_scope(&service, &project).await.expect("a scope");
    assert_eq!(
        db::muted_for(&mut tx, &project, route)
            .await
            .expect("muted"),
        [flaky]
    );
    tx.commit().await.expect("commit");
    assert_eq!(
        send(vec![flaky, steady])
            .await
            .iter()
            .map(|(_, c)| *c)
            .collect::<Vec<_>>(),
        [steady],
        "the muted one hears none of the route's notices"
    );
    assert_eq!(
        routed(
            &service,
            &organization,
            &project,
            "telemetry:rule:b",
            &[flaky]
        )
        .await
        .len(),
        1,
        "another route still reaches it"
    );

    let mut tx = project_scope(&service, &project).await.expect("a scope");
    assert!(
        db::unmute(&mut tx, &project, route, flaky)
            .await
            .expect("unmuted")
    );
    tx.commit().await.expect("commit");
    assert_eq!(send(vec![flaky]).await.len(), 1, "heard again once unmuted");
}

/// The module as the server holds it, with no connector key: no send here
/// opens a connection's sealed fields.
fn module(pool: &PgPool) -> Arc<AppState> {
    Arc::new(AppState {
        db: service_pool(pool, ServiceRole::Notifications),
        config: common::config(),
        service_secrets: ServiceSecrets::new(common::SECRET.to_string(), None::<String>),
        auth: common::AuthStub::new(),
        http: reqwest::Client::new(),
        egress: telmoni_shared::net_guard::Egress::unguarded(reqwest::Client::new()),
        vault: telmoni_shared::envelope::Vault::new(),
        kek: None,
        connectors: telmoni_notifications::connector::Connectors::default(),
        heartbeats: telmoni_shared::heartbeat::Heartbeats::new("notifications", NotificationsLane),
    })
}

/// ⚠ A routed notice answers the connections it was queued to; a replay of its key answers none.
#[sqlx::test]
async fn a_routed_notice_answers_the_connections_it_was_queued_to(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let named = connection(&pool, &project, &organization, "active").await;
    let errored = connection(&pool, &project, &organization, "errored").await;
    let notifier = Notifier(module(&pool));
    let notice = |dedup_key| Notice {
        kind: NotificationKind::IncidentOpened,
        subject_user_id: None,
        title: "Slow calls: alert",
        body: "The p95 duration of model calls over the last hour is 6200 ms.",
        metadata: json!({}),
        dedup_key,
    };
    let route = Route {
        key: "telemetry:rule:a",
        connections: &[named, errored],
        most: 2,
    };

    let routed = notifier
        .emit_routed(
            &organization,
            &project,
            notice(Some("telemetry:incident:a:opened")),
            route,
        )
        .await
        .expect("emitted");
    assert!(!routed.emitted.deduplicated);
    assert_eq!(routed.connections, [named], "not the errored one");

    let replayed = notifier
        .emit_routed(
            &organization,
            &project,
            notice(Some("telemetry:incident:a:opened")),
            route,
        )
        .await
        .expect("replayed");
    assert_eq!(replayed.emitted.feed_id, routed.emitted.feed_id);
    assert!(replayed.emitted.deduplicated);
    assert!(replayed.connections.is_empty(), "a replay queues nothing");

    let reminded = notifier
        .remind(&organization, &project, notice(None), route)
        .await
        .expect("reminded");
    assert_eq!(reminded, [named]);
}

/// ⚠ A routed notice's room goes, by id, to the connections that hear it, and to no other.
#[sqlx::test]
async fn a_routed_notice_spends_its_room_on_the_connections_that_hear_it(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let at = |status, n| connection_as(&pool, &project, &organization, status, Uuid::from_u128(n));
    let errored = at("errored", 1).await;
    let (muted, deaf) = (at("active", 2).await, at("active", 3).await);
    let heard = [at("active", 4).await, at("active", 5).await];
    sqlx::query(
        "INSERT INTO notifications.routes (route, connection_id, project_id, organization_id,
                                           failures, muted_at)
         VALUES ('telemetry:rule:a', $1, $2, $3, 5, now())",
    )
    .bind(muted)
    .bind(&project)
    .bind(&organization)
    .execute(&pool)
    .await
    .expect("a route muted");
    sqlx::query(
        "UPDATE notifications.connections SET event_kinds = '{member_added}' WHERE id = $1",
    )
    .bind(deaf)
    .execute(&pool)
    .await
    .expect("a connection deaf to incidents");
    let notifier = Notifier(module(&pool));
    let notice = Notice {
        kind: NotificationKind::IncidentOpened,
        subject_user_id: None,
        title: "Slow calls: alert",
        body: "The p95 duration of model calls over the last hour is 6200 ms.",
        metadata: json!({}),
        dedup_key: None,
    };
    let all = [heard[1], deaf, errored, heard[0], muted];
    let route = |most| Route {
        key: "telemetry:rule:a",
        connections: &all,
        most,
    };

    let one = notifier
        .remind(&organization, &project, notice.clone(), route(1))
        .await
        .expect("reminded");
    assert_eq!(one, [heard[0]], "the room goes to one that hears it, by id");
    let none = notifier
        .remind(&organization, &project, notice.clone(), route(0))
        .await
        .expect("reminded");
    assert!(none.is_empty(), "no room, no delivery");
    let both = notifier
        .remind(&organization, &project, notice, route(5))
        .await
        .expect("reminded");
    assert_eq!(
        both, heard,
        "the stopped, the muted and the deaf take none of the room"
    );
}

/// A routed notice's metadata is held to the 4 KiB an emit's is, and nothing is written past it.
#[sqlx::test]
async fn a_routed_notice_past_its_metadata_bound_is_refused(pool: PgPool) {
    let (organization, project) = (OrganizationId::new(), ProjectId::new());
    let heard = connection(&pool, &project, &organization, "active").await;
    let notice = Notice {
        kind: NotificationKind::IncidentOpened,
        subject_user_id: None,
        title: "Slow calls: alert",
        body: "The p95 duration of model calls over the last hour is 6200 ms.",
        metadata: json!({ "run": "r".repeat(4_096) }),
        dedup_key: Some("telemetry:incident:a:opened"),
    };
    let route = Route {
        key: "telemetry:rule:a",
        connections: &[heard],
        most: 1,
    };
    let refused = Notifier(module(&pool))
        .emit_routed(&organization, &project, notice, route)
        .await;
    assert!(
        matches!(refused, Err(TelmoniError::PayloadTooLarge { .. })),
        "{refused:?}"
    );
    let written: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM notifications.feed)
              + (SELECT count(*) FROM notifications.deliveries)",
    )
    .fetch_one(&pool)
    .await
    .expect("a count");
    assert_eq!(written, 0);
}
