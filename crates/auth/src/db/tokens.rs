//! `auth.api_tokens` — a project's keys.

use chrono::{DateTime, Utc};
use telmoni_shared::db::tenant_session::{Maintenance, PersonAndOrganization, Project, Scoped};
use telmoni_shared::{OrganizationId, ProjectId, UserId};
use uuid::Uuid;

use crate::db::AuthLane;
use crate::model::ApiToken;

const TOKEN_COLUMNS: &str = "id, project_id, name, description, created_by, \
     expires_at, last_used_at, revoked_at, created_at";

/// Active tokens for `project_id`: not revoked and not expired. A token in its
/// rotation grace window shows here, so a person mid-rotation sees old and new.
pub async fn list(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
) -> sqlx::Result<Vec<ApiToken>> {
    sqlx::query_as::<_, ApiToken>(&format!(
        "SELECT {TOKEN_COLUMNS}
           FROM auth.api_tokens
          WHERE project_id = $1
            AND (revoked_at IS NULL OR revoked_at > now())
            AND (expires_at IS NULL OR expires_at > now())
          ORDER BY created_at DESC, id DESC",
    ))
    .bind(project_id)
    .fetch_all(tx.conn())
    .await
}

/// Mint a token row. The caller hashed the secret and keeps the plaintext
/// for its one answer; nothing here ever reads it back.
#[expect(
    clippy::too_many_arguments,
    reason = "all fields required to insert an API token"
)]
pub async fn create(
    tx: &mut Scoped<'_, Project>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
    name: &str,
    description: Option<&str>,
    created_by: &UserId,
    expires_at: Option<DateTime<Utc>>,
    token_hash: &str,
) -> sqlx::Result<ApiToken> {
    sqlx::query_as::<_, ApiToken>(&format!(
        "INSERT INTO auth.api_tokens
             (id, organization_id, project_id, name, description, token_hash,
              created_by, expires_at, shard_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING {TOKEN_COLUMNS}",
    ))
    .bind(Uuid::now_v7())
    .bind(organization_id)
    .bind(project_id)
    .bind(name)
    .bind(description)
    .bind(token_hash)
    .bind(created_by)
    .bind(expires_at)
    .bind(telmoni_shared::derive_shard_key(organization_id))
    .fetch_one(tx.conn())
    .await
}

/// What a successful validation resolves: the owning organization, what it is
/// called, and the token's own id.
#[derive(Debug)]
pub struct ValidatedToken {
    pub id: Uuid,
    pub organization_id: OrganizationId,
    /// The project the token was minted on: the roster `/v1/members` reads,
    /// since a project admin may mint a key and may see no more than that.
    pub project_id: ProjectId,
    /// The slug the console's paths name the organization by, for
    /// `/v1/organization`'s answer.
    pub slug: String,
    /// The organization's name, and its owner's address, carried because
    /// `/v1` has no BFF to look them up.
    pub name: Option<String>,
    pub owner_email: Option<String>,
    /// The owner's display name, for `/v1/organization`'s answer: the one
    /// column the join did not already carry, which cost that lane a second
    /// transaction of its own.
    pub owner_display_name: Option<String>,
}

impl ValidatedToken {
    /// What to print for the organization: its name.
    #[must_use]
    pub fn label(&self) -> String {
        crate::identity::organization_label(self.name.as_deref())
    }
}

type ValidatedRow = (
    Uuid,
    OrganizationId,
    ProjectId,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Returns the token's id and organization if it is valid: hash matches,
/// neither `revoked_at` nor `expires_at` has passed, and its organization is
/// still active. **Cross-tenant by design**: discovering the organization is
/// the point.
///
/// The status test is the backstop to the deletion's revoke: an organization
/// that asked to be deleted answers no key from the moment it is marked,
/// whatever the revoke reached.
pub async fn validate(
    tx: &mut Scoped<'_, Maintenance<AuthLane>>,
    token_hash: &str,
) -> sqlx::Result<Option<ValidatedToken>> {
    // One round trip on every `/v1` request: the lookup and the touch of
    // `last_used_at` (at most once a minute, so a busy key is not a write
    // per call) in a single statement.
    let row: Option<ValidatedRow> = sqlx::query_as(
        "WITH v AS (
            SELECT t.id, a.external_id, t.project_id, a.slug, a.name, oi.email, oi.display_name
              FROM auth.api_tokens t
              JOIN auth.organizations a ON a.external_id = t.organization_id
              LEFT JOIN auth.organization_members om
                     ON om.organization_id = a.external_id AND om.role = 'owner'
              LEFT JOIN auth.identities oi ON oi.user_id = om.user_id
             WHERE t.token_hash = $1
               AND a.status = 'active'
               AND (t.revoked_at IS NULL OR t.revoked_at > now())
               AND (t.expires_at IS NULL OR t.expires_at > now())
         ), touched AS (
            UPDATE auth.api_tokens SET last_used_at = now()
             WHERE id = (SELECT id FROM v)
               AND (last_used_at IS NULL OR last_used_at < now() - interval '1 minute')
         )
         SELECT id, external_id, project_id, slug, name, email, display_name FROM v",
    )
    .bind(token_hash)
    .fetch_optional(tx.conn())
    .await?;

    Ok(row.map(
        |(id, organization_id, project_id, slug, name, owner_email, owner_display_name)| {
            ValidatedToken {
                id,
                organization_id,
                project_id,
                slug,
                name,
                owner_email,
                owner_display_name,
            }
        },
    ))
}

/// Set `revoked_at = now()` on a token that is still valid — active OR in a
/// rotation grace window. Collapsing a future `revoked_at` is the emergency
/// kill switch: a leaked in-grace token must die now, not report 404 while it
/// keeps validating.
pub async fn revoke_in_project(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    token_id: Uuid,
) -> sqlx::Result<bool> {
    let result = sqlx::query(
        "UPDATE auth.api_tokens SET revoked_at = now()
          WHERE id = $1
            AND project_id = $2
            AND (revoked_at IS NULL OR revoked_at > now())",
    )
    .bind(token_id)
    .bind(project_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Revoke every still-valid token of one organization — the deletion's revoke
/// step — inside the account deletion's transaction. A revoke is auditable
/// where the later cascade is not.
///
/// ⚠ **Project by project, because that is what RLS admits.** `api_tokens`'
/// policy keys on `app.project_id`, so one UPDATE keyed on `organization_id`
/// under an organization scope matched zero rows and every key outlived the
/// deletion request. Each project is bound in turn, then the project GUC is
/// cleared so the transaction goes back to organization work. This is the one
/// query function that changes the binding under `tx`'s type, and it restores
/// it before returning. In-grace tokens (a future `revoked_at`) are collapsed
/// to now, as the single revoke does.
pub async fn revoke_all_in_organization(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
) -> sqlx::Result<u64> {
    let projects: Vec<ProjectId> =
        sqlx::query_scalar("SELECT external_id FROM auth.projects WHERE organization_id = $1")
            .bind(organization_id)
            .fetch_all(tx.conn())
            .await?;
    let mut revoked = 0;
    for project in &projects {
        sqlx::query("SELECT set_config('app.project_id', $1, true)")
            .bind(project)
            .execute(tx.conn())
            .await?;
        revoked += revoke_live_in_project(tx, organization_id, project).await?;
    }
    sqlx::query("SELECT set_config('app.project_id', '', true)")
        .execute(tx.conn())
        .await?;
    Ok(revoked)
}

async fn revoke_live_in_project(
    tx: &mut Scoped<'_, PersonAndOrganization>,
    organization_id: &OrganizationId,
    project_id: &ProjectId,
) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "UPDATE auth.api_tokens SET revoked_at = now()
          WHERE organization_id = $1 AND project_id = $2
            AND (revoked_at IS NULL OR revoked_at > now())",
    )
    .bind(organization_id)
    .bind(project_id)
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

/// What the handler needs back from `begin_rotation` to mint the replacement,
/// read from the row it already holds locked. Not its minter: the replacement
/// is the rotator's.
pub struct RotationContext {
    pub name: String,
    pub description: Option<String>,
}

/// In the caller's transaction, one statement: stamp the old row's
/// `revoked_at` to `now() + grace` and return what the new token needs.
/// `None` (a 404) when it is missing, already revoked, or another project's.
pub async fn begin_rotation(
    tx: &mut Scoped<'_, Project>,
    project_id: &ProjectId,
    token_id: Uuid,
    grace_seconds: i64,
) -> sqlx::Result<Option<RotationContext>> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "UPDATE auth.api_tokens
            SET revoked_at = now() + make_interval(secs => $1::bigint)
          WHERE id = $2 AND project_id = $3 AND (revoked_at IS NULL OR revoked_at > now())
      RETURNING name, description",
    )
    .bind(grace_seconds)
    .bind(token_id)
    .bind(project_id)
    .fetch_optional(tx.conn())
    .await?;
    Ok(row.map(|(name, description)| RotationContext { name, description }))
}

/// Hard-delete tokens seven days past revocation or expiry. The grace gives
/// an oncall human time to ask "why was this revoked?" before the row goes.
/// Driven by the nightly retention sweep.
pub async fn delete_expired(tx: &mut Scoped<'_, Maintenance<AuthLane>>) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "DELETE FROM auth.api_tokens
          WHERE (revoked_at IS NOT NULL AND revoked_at < now() - interval '7 days')
             OR (expires_at IS NOT NULL AND expires_at < now() - interval '7 days')",
    )
    .execute(tx.conn())
    .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use sqlx::PgPool;
    use telmoni_shared::db::tenant_session::{maintenance_scope, organization_scope};

    fn oid(s: &str) -> telmoni_shared::OrganizationId {
        telmoni_shared::OrganizationId::try_new(s).expect("valid test organization id")
    }

    fn pid(s: &str) -> telmoni_shared::ProjectId {
        telmoni_shared::ProjectId::try_new(s).expect("valid test project id")
    }

    fn uid(s: &str) -> telmoni_shared::UserId {
        telmoni_shared::UserId::try_new(s).expect("valid test user id")
    }

    /// The organization a token hangs off, and its first project, seeded under
    /// `organization_scope` because that policy is what admits a new project.
    async fn seed_organization(pool: &PgPool) {
        let organization = oid("org_tok");
        let project = pid("project-tok");
        let mut tx = organization_scope(pool, &organization)
            .await
            .expect("scope");
        crate::db::organizations::create(&mut tx, &organization)
            .await
            .unwrap();
        crate::db::projects::create(&mut tx, &project, &organization, "Default project")
            .await
            .unwrap();
        tx.commit().await.expect("commit");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_create_and_list_token(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "my-key",
            None,
            &uid("user_tok"),
            None,
            "hash-abc",
        )
        .await?;
        assert_eq!(tok.project_id, pid("project-tok").as_str());
        assert_eq!(tok.name, "my-key");
        assert!(tok.expires_at.is_none());

        let list = list(&mut tx, &pid("project-tok")).await?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, tok.id);
        tx.commit().await?;
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_validate_active_token(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "valid",
            None,
            &uid("user_tok"),
            None,
            "hash-valid",
        )
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        let result = validate(&mut tx, "hash-valid").await?;
        let validated = result.expect("active token validates");
        assert_eq!(validated.organization_id.as_str(), "org_tok");
        assert_eq!(
            validated.id, tok.id,
            "validation resolves the token's own id"
        );
        tx.commit().await?;
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_validate_unknown_hash_returns_none(pool: PgPool) -> sqlx::Result<()> {
        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        let result = validate(&mut tx, "no-such-hash").await?;
        assert!(result.is_none());
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_revoke_makes_token_invalid(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "revoke-me",
            None,
            &uid("user_tok"),
            None,
            "hash-revoke",
        )
        .await?;
        let revoked = revoke_in_project(&mut tx, &pid("project-tok"), tok.id).await?;
        assert!(revoked);
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        let result = validate(&mut tx, "hash-revoke").await?;
        assert!(result.is_none(), "revoked token must not validate");
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_revoke_wrong_organization_returns_false(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "key",
            None,
            &uid("user_tok"),
            None,
            "hash-x",
        )
        .await?;
        let revoked = revoke_in_project(&mut tx, &pid("project-wrong"), tok.id).await?;
        assert!(!revoked);
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_validate_excludes_expired_token(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let past = Utc::now() - ChronoDuration::seconds(1);
        create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "expired",
            None,
            &uid("user_tok"),
            Some(past),
            "hash-expired",
        )
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert!(validate(&mut tx, "hash-expired").await?.is_none());
        let mut tx2 =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        assert!(list(&mut tx2, &pid("project-tok")).await?.is_empty());
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_rotate_mints_new_grace_revokes_old(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let old = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "rotate-me",
            Some("nightly CI"),
            &uid("user_tok"),
            None,
            "hash-old",
        )
        .await?;

        let ctx = begin_rotation(&mut tx, &pid("project-tok"), old.id, 3600)
            .await?
            .expect("rotation context");
        assert_eq!(ctx.name, "rotate-me");
        assert_eq!(ctx.description.as_deref(), Some("nightly CI"));

        let _new = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            &ctx.name,
            ctx.description.as_deref(),
            &uid("user_tok"),
            None,
            "hash-new",
        )
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert!(
            validate(&mut tx, "hash-old").await?.is_some(),
            "old token must validate during grace window"
        );
        assert!(validate(&mut tx, "hash-new").await?.is_some());
        tx.commit().await?;

        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        assert_eq!(list(&mut tx, &pid("project-tok")).await?.len(), 2);

        sqlx::query(
            "UPDATE auth.api_tokens SET revoked_at = now() - interval '1 second' WHERE id = $1",
        )
        .bind(old.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert!(validate(&mut tx, "hash-old").await?.is_none());
        assert!(validate(&mut tx, "hash-new").await?.is_some());
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_revoke_kills_an_in_grace_token(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let old = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "leaked",
            None,
            &uid("user_tok"),
            None,
            "hash-grace",
        )
        .await?;
        begin_rotation(&mut tx, &pid("project-tok"), old.id, 3600)
            .await?
            .expect("rotation context");
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert!(
            validate(&mut tx, "hash-grace").await?.is_some(),
            "an in-grace token validates until the window closes"
        );
        tx.commit().await?;

        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        assert!(
            revoke_in_project(&mut tx, &pid("project-tok"), old.id).await?,
            "revoke must collapse the grace window, not miss the row"
        );
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert!(
            validate(&mut tx, "hash-grace").await?.is_none(),
            "a revoked in-grace token is immediately dead"
        );
        tx.commit().await?;
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_begin_rotation_returns_none_for_unknown_or_revoked(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "k",
            None,
            &uid("user_tok"),
            None,
            "hash-r",
        )
        .await?;

        assert!(
            begin_rotation(&mut tx, &pid("project-other"), tok.id, 60)
                .await?
                .is_none()
        );

        revoke_in_project(&mut tx, &pid("project-tok"), tok.id).await?;
        assert!(
            begin_rotation(&mut tx, &pid("project-tok"), tok.id, 60)
                .await?
                .is_none()
        );
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_delete_expired_hard_deletes_past_seven_day_window(
        pool: PgPool,
    ) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let keep = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "keep",
            None,
            &uid("user_tok"),
            None,
            "hash-keep",
        )
        .await?;
        let gone = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "gone1",
            None,
            &uid("user_tok"),
            None,
            "hash-gone1",
        )
        .await?;
        let gone2 = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "gone2",
            None,
            &uid("user_tok"),
            None,
            "hash-gone2",
        )
        .await?;

        sqlx::query(
            "UPDATE auth.api_tokens SET revoked_at = now() - interval '8 days' WHERE id = $1",
        )
        .bind(gone.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE auth.api_tokens SET expires_at = now() - interval '8 days' WHERE id = $1",
        )
        .bind(gone2.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        let deleted = delete_expired(&mut tx).await?;
        assert_eq!(deleted, 2);
        tx.commit().await?;

        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let row: (Uuid,) = sqlx::query_as("SELECT id FROM auth.api_tokens WHERE project_id = $1")
            .bind(pid("project-tok"))
            .fetch_one(&mut *tx)
            .await?;
        assert_eq!(row.0, keep.id);
        Ok(())
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_delete_expired_keeps_recently_revoked(pool: PgPool) -> sqlx::Result<()> {
        seed_organization(&pool).await;
        let mut tx =
            telmoni_shared::db::tenant_session::project_scope(&pool, &pid("project-tok")).await?;
        let tok = create(
            &mut tx,
            &oid("org_tok"),
            &pid("project-tok"),
            "recent",
            None,
            &uid("user_tok"),
            None,
            "hash-recent",
        )
        .await?;
        sqlx::query(
            "UPDATE auth.api_tokens SET revoked_at = now() - interval '1 day' WHERE id = $1",
        )
        .bind(tok.id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        let mut tx = maintenance_scope(&pool, AuthLane).await?;
        assert_eq!(delete_expired(&mut tx).await?, 0);
        Ok(())
    }
}
