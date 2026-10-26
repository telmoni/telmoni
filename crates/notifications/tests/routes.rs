//! An alert's notices as this module routes them: to the connections their
//! route names that are the project's, active, take the kind and are not
//! muted for the route. A landed delivery resets its connection's run under
//! the route, one that fails for good adds to it, and five in a row mute the
//! connection for that route alone, until a member unmutes it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

use sqlx::PgPool;
use uuid::Uuid;

use telmoni_notifications::db::{self, NotificationsLane};
use telmoni_shared::db::tenant_session::{maintenance_scope, project_scope};
use telmoni_shared::test_util::{ServiceRole, service_pool};
use telmoni_shared::{NotificationKind, OrganizationId, ProjectId};

/// A webhook connection of `project`, at `status`, seeded as its owner would
/// leave it, its sealed fields placeholders no send here opens.
async fn connection(
    pool: &PgPool,
    project: &ProjectId,
    organization: &OrganizationId,
    status: &str,
) -> Uuid {
    let id = Uuid::new_v4();
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

/// The connections `route` sends an incident's opening to, among `targets`.
async fn routed(
    service: &PgPool,
    organization: &OrganizationId,
    project: &ProjectId,
    route: &str,
    targets: &[Uuid],
) -> Vec<(Uuid, Uuid)> {
    let mut tx = project_scope(service, project).await.expect("a scope");
    let queued = db::enqueue_routed(
        &mut tx,
        organization,
        project,
        NotificationKind::IncidentOpened,
        "Slow calls: alert",
        "The p95 duration of model calls over the last hour is 6200 ms.",
        route,
        targets,
    )
    .await
    .expect("queued");
    let sent: Vec<(Uuid, Uuid)> = sqlx::query_as(
        "SELECT id, connection_id FROM notifications.deliveries WHERE id = ANY($1)
          ORDER BY connection_id",
    )
    .bind(&queued)
    .fetch_all(tx.conn())
    .await
    .expect("the deliveries");
    tx.commit().await.expect("commit");
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
