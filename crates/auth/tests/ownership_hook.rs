//! A deployment's say on who owns what ([`OwnershipHook`]) and on what an
//! organization may take in ([`Entitlements`]): asked before an organization
//! is offered or taken, told once one is founded or changes hands, and read
//! before an invitation is sent. With neither linked, nothing is asked: every
//! other suite here runs without them.
#![allow(
    clippy::indexing_slicing,
    reason = "a panicking helper is a failing test"
)]
#![expect(
    clippy::unwrap_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use telmoni_auth::test_provider::{ScriptedProvider, as_person};
use telmoni_auth::{AppState, Config, Siblings, router};
use telmoni_shared::seam::{Entitlements, OwnershipChange, OwnershipHook};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{Dormant, Grants, OrganizationId, PlanError, TelmoniError, UserId};

const SERVICE_SECRET: &str = "test-service-secret";
const OWNER: &str = "user_hook_owner";
const ADMIN: &str = "user_hook_admin";
const STRANGER: &str = "user_hook_stranger";

/// What the hook was asked and told, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Heard {
    Asked {
        organization: String,
        recipient: String,
    },
    Told(OwnershipChange),
}

/// An ownership hook that records everything and refuses whoever is in
/// `refused`.
#[derive(Default)]
struct RecordingOwnership {
    heard: Mutex<Vec<Heard>>,
    refused: Mutex<HashSet<String>>,
}

impl RecordingOwnership {
    fn heard(&self) -> Vec<Heard> {
        self.heard.lock().unwrap().clone()
    }

    fn told(&self) -> Vec<OwnershipChange> {
        self.heard()
            .into_iter()
            .filter_map(|heard| match heard {
                Heard::Told(change) => Some(change),
                Heard::Asked { .. } => None,
            })
            .collect()
    }

    fn refuse(&self, recipient: &str, refused: bool) {
        let mut set = self.refused.lock().unwrap();
        if refused {
            set.insert(recipient.to_owned());
        } else {
            set.remove(recipient);
        }
    }
}

/// The refusal a deployment answers, in its own words.
fn limit_reached() -> TelmoniError {
    PlanError::LimitReached {
        entitlement: "pro".into(),
        detail: "Its new owner already holds what a second would take.".into(),
    }
    .into()
}

#[async_trait::async_trait]
impl OwnershipHook for RecordingOwnership {
    async fn may_take_ownership(
        &self,
        organization_id: &OrganizationId,
        recipient: &UserId,
    ) -> Result<(), TelmoniError> {
        self.heard.lock().unwrap().push(Heard::Asked {
            organization: organization_id.to_string(),
            recipient: recipient.to_string(),
        });
        if self.refused.lock().unwrap().contains(recipient.as_str()) {
            return Err(limit_reached());
        }
        Ok(())
    }

    async fn ownership_changed(&self, change: OwnershipChange) -> Result<(), TelmoniError> {
        self.heard.lock().unwrap().push(Heard::Told(change));
        Ok(())
    }
}

/// Entitlements under which the organizations in `dormant` take nothing new.
#[derive(Default)]
struct DormantEntitlements {
    dormant: Mutex<HashSet<String>>,
}

#[async_trait::async_trait]
impl Entitlements for DormantEntitlements {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        if self
            .dormant
            .lock()
            .unwrap()
            .contains(organization_id.as_str())
        {
            return Ok(Grants {
                dormant: Some(Dormant {
                    entitlement: "pro".into(),
                    detail: "This organization is dormant.".into(),
                }),
            });
        }
        Ok(Grants::unlimited())
    }
}

/// The two doubles a test reads back.
#[derive(Clone, Default)]
struct Deployment {
    ownership: Arc<RecordingOwnership>,
    entitlements: Arc<DormantEntitlements>,
}

impl Deployment {
    fn app(&self, pool: &PgPool) -> Router {
        let config = Config {
            database_url: String::new(),
            service_secret: SERVICE_SECRET.into(),
            service_secret_next: None,
            allow_test_session: false,
            redirect_uri: "http://localhost:3000/auth/callback".into(),
            app_url: "http://localhost:3000".into(),
            mail_from: "Telmoni <test@example.com>".into(),
            support_email: None,
            deletion_tail_budget_ms: 8_000,
        };
        let db = service_pool(pool, ServiceRole::Auth);
        router(Arc::new(AppState {
            issuer: telmoni_auth::test_provider::test_issuer(db.clone()),
            password: None,
            external: Some(telmoni_auth::test_provider::external(Arc::new(
                ScriptedProvider::new(),
            ))),
            db,
            config,
            siblings: Siblings {
                ownership_hook: Some(self.ownership.clone()),
                entitlements: Some(self.entitlements.clone()),
                ..Siblings::default()
            },
            mailer: Arc::new(telmoni_auth::mailer::ComposingMailer::new(Arc::new(
                telmoni_shared::mail::NoopSender,
            ))),
            exchange_cache: telmoni_auth::ExchangeCache::new(),
        }))
    }

    /// One request as `caller`, standing in `organization` when given.
    async fn call(
        &self,
        pool: &PgPool,
        method: &str,
        uri: &str,
        caller: &str,
        organization: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header("x-service-secret", SERVICE_SECRET)
            .header("content-type", "application/json");
        if let Some(organization) = organization {
            req = req.header("x-organization-id", organization);
        }
        let req = as_person(req, pool, caller)
            .await
            .body(Body::from(body.unwrap_or_else(|| json!({})).to_string()))
            .unwrap();
        let resp = self.app(pool).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let body = if bytes.is_empty() {
            json!(null)
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, body)
    }

    /// Sign somebody in for the first time; answers the organization they
    /// were provisioned with.
    async fn sign_in(&self, pool: &PgPool, user: &str) -> String {
        seed_identity(pool, user, &format!("{user}@example.test")).await;
        let (status, body) = self.call(pool, "POST", "/me", user, None, None).await;
        assert_eq!(status, StatusCode::OK, "sign-in failed for {user}: {body}");
        body["activeOrganizationId"].as_str().unwrap().to_owned()
    }

    /// Bring `user` into the owner's organization as an admin, as the product
    /// does: invited, then accepted.
    async fn admit_admin(&self, pool: &PgPool, organization: &str, user: &str) {
        let (status, body) = self
            .call(
                pool,
                "POST",
                "/internal/organization/invites",
                OWNER,
                Some(organization),
                Some(json!({ "email": format!("{user}@example.test"), "role": "admin" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "invite {user}: {body}");
        let invite = body["id"].as_str().unwrap().to_owned();
        let (status, body) = self
            .call(
                pool,
                "POST",
                &format!("/internal/me/invites/{invite}/accept"),
                user,
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "accept for {user}: {body}");
    }
}

fn founded(organization: &str, owner: &str) -> OwnershipChange {
    OwnershipChange::Founded {
        organization_id: OrganizationId::try_new(organization).unwrap(),
        owner: UserId::try_new(owner).unwrap(),
    }
}

/// The organization's owner, read straight from the roster.
async fn owner_of(pool: &PgPool, organization: &str) -> String {
    sqlx::query_scalar(
        "SELECT user_id FROM auth.organization_members
          WHERE organization_id = $1 AND role = 'owner'",
    )
    .bind(organization)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn the_hook_is_told_of_every_founding_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();

    let provisioned = deployment.sign_in(&pool, OWNER).await;
    assert_eq!(
        deployment.ownership.told(),
        vec![founded(&provisioned, OWNER)]
    );

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organizations",
            OWNER,
            None,
            Some(json!({ "name": "Second" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let second = body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        deployment.ownership.told(),
        vec![founded(&provisioned, OWNER), founded(&second, OWNER)],
        "the hook was told of a founding other than once"
    );

    // `/me` again finds them in an organization, and founds nothing.
    let (status, _) = deployment
        .call(&pool, "POST", "/me", OWNER, None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deployment.ownership.told().len(), 2);
}

#[sqlx::test]
async fn a_handover_is_asked_before_the_offer_and_the_accept_and_told_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, ADMIN).await;
    deployment.admit_admin(&pool, &organization, ADMIN).await;
    let before = deployment.ownership.heard().len();

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer",
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": ADMIN })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer/accept",
            ADMIN,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let asked = Heard::Asked {
        organization: organization.clone(),
        recipient: ADMIN.to_owned(),
    };
    assert_eq!(
        deployment.ownership.heard()[before..],
        [
            asked.clone(),
            asked,
            Heard::Told(OwnershipChange::HandedOver {
                organization_id: OrganizationId::try_new(&organization).unwrap(),
                from: Some(UserId::try_new(OWNER).unwrap()),
                to: UserId::try_new(ADMIN).unwrap(),
            }),
        ],
        "asked at the offer and at the accept, and told once, after"
    );
    assert_eq!(owner_of(&pool, &organization).await, ADMIN);
}

/// ⚠ The deployment's refusal is the person's answer, at either step, and a
/// refused accept changes nobody's role and tells the hook nothing.
#[sqlx::test]
async fn a_refusal_stops_the_offer_and_the_accept(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, ADMIN).await;
    deployment.admit_admin(&pool, &organization, ADMIN).await;

    deployment.ownership.refuse(ADMIN, true);
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer",
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": ADMIN })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["entitlement"], "pro");
    let offered: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.organization_members
          WHERE organization_id = $1 AND transfer_offered_at IS NOT NULL",
    )
    .bind(&organization)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offered, 0, "a refused offer was made all the same");

    // Made while the deployment would allow it, then refused at the accept.
    deployment.ownership.refuse(ADMIN, false);
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer",
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": ADMIN })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    deployment.ownership.refuse(ADMIN, true);
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer/accept",
            ADMIN,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(owner_of(&pool, &organization).await, OWNER);
    assert!(
        !deployment
            .ownership
            .told()
            .iter()
            .any(|change| matches!(change, OwnershipChange::HandedOver { .. })),
        "the hook was told of a handover that did not happen"
    );
}

/// Somebody who could not make the offer is refused by auth's own rules, and
/// learns nothing of the deployment's answer.
#[sqlx::test]
async fn only_the_owner_offering_to_an_admin_is_shown_the_hooks_answer(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, ADMIN).await;
    deployment.sign_in(&pool, STRANGER).await;
    deployment.admit_admin(&pool, &organization, ADMIN).await;
    deployment.ownership.refuse(ADMIN, true);
    deployment.ownership.refuse(STRANGER, true);
    let before = deployment.ownership.heard().len();

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer",
            STRANGER,
            Some(&organization),
            Some(json!({ "memberId": ADMIN })),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer",
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": STRANGER })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an offer to somebody off the roster: {body}"
    );
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer/accept",
            STRANGER,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    // An admin holding no offer is answered by auth: nothing to accept.
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/owner-transfer/accept",
            ADMIN,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        deployment.ownership.heard().len(),
        before,
        "the hook was asked on behalf of somebody who could not make the change"
    );
}

#[sqlx::test]
async fn a_dormant_organization_sends_no_invitation(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/projects",
            OWNER,
            Some(&organization),
            Some(json!({ "name": "Payments" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let project = body["id"].as_str().unwrap().to_owned();
    deployment
        .entitlements
        .dormant
        .lock()
        .unwrap()
        .insert(organization.clone());

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/organization/invites",
            OWNER,
            Some(&organization),
            Some(json!({ "email": "someone@example.test", "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["detail"], "This organization is dormant.");

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            &format!("/internal/projects/{project}/invites"),
            OWNER,
            Some(&organization),
            Some(json!({ "email": "someone@example.test", "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    let invites: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM auth.organization_invites)
              + (SELECT count(*) FROM auth.member_invites)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(invites, 0, "a refused invitation was written");

    // Reads, and everything else that takes nothing in, stand.
    let (status, body) = deployment
        .call(
            &pool,
            "GET",
            "/internal/organization/members",
            OWNER,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}
