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

use std::collections::{HashMap, HashSet};
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
use telmoni_shared::seam::{
    Auth, Entitlements, OrganizationFounding, OwnershipChange, OwnershipHook,
};
use telmoni_shared::test_util::{ServiceRole, apply_audit_migrations, seed_identity, service_pool};
use telmoni_shared::{
    Flag, Grants, MemberCeiling, OrganizationId, Paused, PlanError, TelmoniError, UserId,
};

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
    /// Whether `founder` may found another organization at their asking.
    AskedToFound(String),
    Told(OwnershipChange),
}

/// An ownership hook that records everything and refuses whoever is in
/// `refused`.
#[derive(Default)]
struct RecordingOwnership {
    heard: Mutex<Vec<Heard>>,
    refused: Mutex<HashSet<String>>,
    /// Whoever is refused a second organization at their asking.
    refused_founding: Mutex<HashSet<String>>,
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
                Heard::Asked { .. } | Heard::AskedToFound(_) => None,
            })
            .collect()
    }

    fn asked_to_found(&self) -> Vec<String> {
        self.heard()
            .into_iter()
            .filter_map(|heard| match heard {
                Heard::AskedToFound(founder) => Some(founder),
                Heard::Asked { .. } | Heard::Told(_) => None,
            })
            .collect()
    }

    fn refuse_founding(&self, founder: &str, refused: bool) {
        let mut set = self.refused_founding.lock().unwrap();
        if refused {
            set.insert(founder.to_owned());
        } else {
            set.remove(founder);
        }
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

    async fn may_found(&self, founder: &UserId) -> Result<(), TelmoniError> {
        self.heard
            .lock()
            .unwrap()
            .push(Heard::AskedToFound(founder.to_string()));
        if self
            .refused_founding
            .lock()
            .unwrap()
            .contains(founder.as_str())
        {
            return Err(limit_reached());
        }
        Ok(())
    }
}

/// Entitlements under which the organizations in `paused` take nothing new,
/// and each in `ceilings` holds at most so many people.
#[derive(Default)]
struct PausedEntitlements {
    paused: Mutex<HashSet<String>>,
    ceilings: Mutex<HashMap<String, u32>>,
}

impl PausedEntitlements {
    fn hold(&self, organization: &str, limit: u32) {
        self.ceilings
            .lock()
            .unwrap()
            .insert(organization.to_owned(), limit);
    }
}

/// The sentence the double refuses a seat with.
fn holds(limit: u32) -> String {
    format!("Hobby holds {limit}.")
}

#[async_trait::async_trait]
impl Entitlements for PausedEntitlements {
    async fn grants(&self, organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        if self
            .paused
            .lock()
            .unwrap()
            .contains(organization_id.as_str())
        {
            return Ok(Grants {
                paused: Some(Paused {
                    entitlement: "pro".into(),
                    detail: "This organization is paused.".into(),
                }),
            });
        }
        Ok(Grants::unlimited())
    }

    async fn member_ceiling(
        &self,
        organization_id: &OrganizationId,
    ) -> Result<Option<MemberCeiling>, TelmoniError> {
        Ok(self
            .ceilings
            .lock()
            .unwrap()
            .get(organization_id.as_str())
            .map(|&limit| MemberCeiling {
                limit,
                entitlement: "pro".into(),
                detail: holds(limit),
            }))
    }
}

/// The two doubles a test reads back.
#[derive(Clone, Default)]
struct Deployment {
    ownership: Arc<RecordingOwnership>,
    entitlements: Arc<PausedEntitlements>,
}

impl Deployment {
    fn app(&self, pool: &PgPool) -> Router {
        router(self.state(pool))
    }

    /// Auth's state over `pool`, which the seam is answered on.
    fn state(&self, pool: &PgPool) -> Arc<AppState> {
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
        Arc::new(AppState {
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
        })
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
        let (status, body) = self
            .call(pool, "POST", "/internal/v1/me", user, None, None)
            .await;
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
                "/internal/v1/organization/invites",
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
                &format!("/internal/v1/me/invites/{invite}/accept"),
                user,
                None,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::OK, "accept for {user}: {body}");
    }
}

/// What `make flag` writes: one append-only global row.
async fn flip(pool: &PgPool, flag: Flag, on: bool) {
    sqlx::query(
        "INSERT INTO auth.feature_flags (key, enabled, note, actor) VALUES ($1, $2, 'test', 'test')",
    )
    .bind(flag.as_str())
    .bind(on)
    .execute(pool)
    .await
    .unwrap();
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
            "/internal/v1/organizations",
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
        .call(&pool, "POST", "/internal/v1/me", OWNER, None, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deployment.ownership.told().len(), 2);
}

/// A second organization at the person's asking is the hook's to refuse —
/// a deployment that has it paid for first does — and the person reads the
/// hook's words; the first, provisioned by `/me`, is never asked about.
#[sqlx::test]
async fn a_founding_at_the_persons_asking_is_the_hooks_to_refuse_and_the_first_is_not(
    pool: PgPool,
) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    deployment.ownership.refuse_founding(OWNER, true);

    let provisioned = deployment.sign_in(&pool, OWNER).await;
    assert!(
        deployment.ownership.asked_to_found().is_empty(),
        "the hook was asked about a person's first organization"
    );

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/v1/organizations",
            OWNER,
            None,
            Some(json!({ "name": "Second" })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["entitlement"], "pro");
    assert_eq!(
        body["detail"],
        "Its new owner already holds what a second would take."
    );
    assert_eq!(
        deployment.ownership.asked_to_found(),
        vec![OWNER.to_owned()]
    );
    assert_eq!(
        deployment.ownership.told(),
        vec![founded(&provisioned, OWNER)],
        "a refused founding was told of"
    );

    deployment.ownership.refuse_founding(OWNER, false);
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/v1/organizations",
            OWNER,
            None,
            Some(json!({ "name": "Second" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(deployment.ownership.asked_to_found().len(), 2);
}

/// A deployment founds an organization itself, under an id it minted, once:
/// the row, the owner and the hook told as the person's own asking would;
/// founded again under the same id, answered as it stands; a URL asked for
/// that is taken, derived from the name rather than refused; and a name or
/// URL the person's asking would refuse, refused before anything is taken.
#[sqlx::test]
async fn the_deployment_founds_an_organization_under_its_own_id_once(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let state = deployment.state(&pool);
    let provisioned = deployment.sign_in(&pool, OWNER).await;

    let minted = OrganizationId::new();
    let founding = OrganizationFounding {
        organization_id: minted.clone(),
        founder: UserId::try_new(OWNER).unwrap(),
        name: "Paid Labs".into(),
        slug: Some("paid-labs".into()),
    };
    let founded_first = state.found_organization(&founding).await.unwrap();
    assert_eq!(founded_first.organization_id, minted);
    assert_eq!(founded_first.slug, "paid-labs");
    assert_eq!(owner_of(&pool, minted.as_str()).await, OWNER);
    assert_eq!(
        deployment.ownership.told(),
        vec![
            founded(&provisioned, OWNER),
            founded(minted.as_str(), OWNER)
        ]
    );

    let again = state.found_organization(&founding).await.unwrap();
    assert_eq!(again.slug, "paid-labs");
    assert_eq!(
        deployment.ownership.told().len(),
        2,
        "a founding under an id already founded was told of again"
    );

    // The URL asked for is taken now: derived from the name, never refused.
    let other = OrganizationFounding {
        organization_id: OrganizationId::new(),
        ..founding.clone()
    };
    let derived = state.found_organization(&other).await.unwrap();
    assert_ne!(derived.slug, "paid-labs");
    assert!(
        derived.slug.starts_with("paid-labs") || derived.slug.starts_with("org-"),
        "{}",
        derived.slug
    );

    for (name, slug) in [("   ", None), ("Acme", Some("Bad Slug"))] {
        assert!(
            state.check_founding(name, slug).await.is_err(),
            "{name:?} {slug:?} passed the check"
        );
    }
    assert!(state.check_founding("Acme", Some("acme")).await.is_ok());

    // New organizations closed: the check refuses, before the deployment
    // takes anything of the founder; a founding already paid for is owed,
    // and founded all the same.
    flip(&pool, Flag::Signup, false).await;
    let closed = state
        .check_founding("Acme", None)
        .await
        .expect_err("a founding was checked fine with new organizations closed")
        .to_problem_details();
    assert_eq!(closed.status, 503);
    assert_eq!(closed.type_uri, "/errors/tenant/feature-off");
    let owed = OrganizationFounding {
        organization_id: OrganizationId::new(),
        name: "Owed".into(),
        slug: None,
        ..founding.clone()
    };
    assert!(
        state.found_organization(&owed).await.is_ok(),
        "a paid founding was refused for new organizations closed since"
    );
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
            "/internal/v1/organization/owner-transfer",
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
            "/internal/v1/organization/owner-transfer/accept",
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
            "/internal/v1/organization/owner-transfer",
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
            "/internal/v1/organization/owner-transfer",
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
            "/internal/v1/organization/owner-transfer/accept",
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
            "/internal/v1/organization/owner-transfer",
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
            "/internal/v1/organization/owner-transfer",
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
            "/internal/v1/organization/owner-transfer/accept",
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
            "/internal/v1/organization/owner-transfer/accept",
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
async fn a_paused_organization_sends_no_invitation(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/v1/projects",
            OWNER,
            Some(&organization),
            Some(json!({ "name": "Payments" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let project = body["id"].as_str().unwrap().to_owned();
    deployment
        .entitlements
        .paused
        .lock()
        .unwrap()
        .insert(organization.clone());

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/v1/organization/invites",
            OWNER,
            Some(&organization),
            Some(json!({ "email": "someone@example.test", "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["detail"], "This organization is paused.");

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            &format!("/internal/v1/projects/{project}/invites"),
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
            "/internal/v1/organization/members",
            OWNER,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ceiling"], Value::Null, "a ceiling nobody set: {body}");
}

/// A paused organization gives no seat from its roster, as it sends no
/// invitation: the seat takes nobody new in, and still the pause stands.
#[sqlx::test]
async fn a_paused_organization_gives_no_seat(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    let project = deployment.project(&pool, &organization, "Payments").await;
    let member = "user_member";
    deployment.enrol(&pool, &organization, member).await;
    deployment
        .entitlements
        .paused
        .lock()
        .unwrap()
        .insert(organization.clone());

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            &format!("/internal/v1/projects/{project}/members"),
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": member, "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    let seats: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth.project_members WHERE project_id = $1 AND user_id = $2",
    )
    .bind(&project)
    .bind(member)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(seats, 0, "a refused seat was written");
}

/// At its member ceiling an organization still seats one of its members on a
/// project: the roster the ceiling counts gains nobody, so it is not asked.
#[sqlx::test]
async fn an_organization_at_its_ceiling_still_seats_its_members(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    let project = deployment.project(&pool, &organization, "Payments").await;
    let member = "user_member";
    deployment.enrol(&pool, &organization, member).await;
    deployment.entitlements.hold(&organization, 1);

    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            &format!("/internal/v1/projects/{project}/members"),
            OWNER,
            Some(&organization),
            Some(json!({ "memberId": member, "role": "member" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(seated(&pool, &organization).await, 2, "the roster grew");
}

/// How many people the organization holds, read straight from the roster.
async fn seated(pool: &PgPool, organization: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM auth.organization_members WHERE organization_id = $1")
        .bind(organization)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// How many invitations, to the organization or to its projects, are live.
async fn live_invitations(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM auth.organization_invites
                  WHERE accepted_at IS NULL AND revoked_at IS NULL)
              + (SELECT count(*) FROM auth.member_invites
                  WHERE accepted_at IS NULL AND revoked_at IS NULL)",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

impl Deployment {
    /// A project of the owner's, in `organization`.
    async fn project(&self, pool: &PgPool, organization: &str, name: &str) -> String {
        let (status, body) = self
            .call(
                pool,
                "POST",
                "/internal/v1/projects",
                OWNER,
                Some(organization),
                Some(json!({ "name": name })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().unwrap().to_owned()
    }

    /// An invitation to the organization, as the owner; what auth answered.
    async fn invite(&self, pool: &PgPool, organization: &str, email: &str) -> (StatusCode, Value) {
        self.call(
            pool,
            "POST",
            "/internal/v1/organization/invites",
            OWNER,
            Some(organization),
            Some(json!({ "email": email, "role": "member" })),
        )
        .await
    }

    /// An invitation to a project, as the owner; what auth answered.
    async fn invite_to(
        &self,
        pool: &PgPool,
        organization: &str,
        project: &str,
        email: &str,
    ) -> (StatusCode, Value) {
        self.call(
            pool,
            "POST",
            &format!("/internal/v1/projects/{project}/invites"),
            OWNER,
            Some(organization),
            Some(json!({ "email": email, "role": "member" })),
        )
        .await
    }

    /// `user` signed in and on the organization's roster as a Member, by its
    /// invitation accepted.
    async fn enrol(&self, pool: &PgPool, organization: &str, user: &str) {
        self.sign_in(pool, user).await;
        let (status, body) = self
            .invite(pool, organization, &format!("{user}@example.test"))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        let invite = body["id"].as_str().unwrap().to_owned();
        let (status, body) = self.accept(pool, user, &invite).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    /// `user` accepts the invitation `invite` from the console's bell.
    async fn accept(&self, pool: &PgPool, user: &str, invite: &str) -> (StatusCode, Value) {
        self.call(
            pool,
            "POST",
            &format!("/internal/v1/me/invites/{invite}/accept"),
            user,
            None,
            None,
        )
        .await
    }

    /// What the roster page reads as the organization's ceiling.
    async fn ceiling_read(&self, pool: &PgPool, organization: &str, uri: &str) -> Value {
        let (status, body) = self
            .call(pool, "GET", uri, OWNER, Some(organization), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["ceiling"].clone()
    }
}

/// The 402 a seat past the ceiling is refused with, in the deployment's words.
fn assert_past_the_ceiling(status: StatusCode, body: &Value, limit: u32) {
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["type"], "/errors/plan/limit-reached");
    assert_eq!(body["entitlement"], "pro");
    assert_eq!(body["detail"], holds(limit));
}

/// One member on Hobby, as the hosted service holds it: at its ceiling an
/// organization promises no seat on either lane and writes no invitation,
/// and both roster pages are told the ceiling and what stands against it —
/// whoever could invite is; a member of the project, who reads its roster
/// and nothing of the organization's, is not.
#[sqlx::test]
async fn an_organization_at_its_ceiling_promises_no_seat_and_its_rosters_say_so(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, STRANGER).await;
    let project = deployment.project(&pool, &organization, "Payments").await;
    // Seated while there was room: the roster's one member besides the owner.
    let (status, body) = deployment
        .invite_to(
            &pool,
            &organization,
            &project,
            &format!("{STRANGER}@example.test"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let seat = body["id"].as_str().unwrap().to_owned();
    let (status, body) = deployment.accept(&pool, STRANGER, &seat).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    deployment.entitlements.hold(&organization, 1);

    let (status, body) = deployment
        .invite(&pool, &organization, "someone@example.test")
        .await;
    assert_past_the_ceiling(status, &body, 1);
    let (status, body) = deployment
        .invite_to(&pool, &organization, &project, "someone@example.test")
        .await;
    assert_past_the_ceiling(status, &body, 1);
    assert_eq!(
        live_invitations(&pool).await,
        0,
        "a refused invitation was written"
    );

    let expected = json!({ "limit": 1, "held": 2, "reached": true, "detail": holds(1) });
    assert_eq!(
        deployment
            .ceiling_read(&pool, &organization, "/internal/v1/organization/members")
            .await,
        expected
    );
    let roster = format!("/internal/v1/projects/{project}/members");
    assert_eq!(
        deployment.ceiling_read(&pool, &organization, &roster).await,
        expected
    );
    let (status, body) = deployment
        .call(&pool, "GET", &roster, STRANGER, Some(&organization), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["ceiling"],
        Value::Null,
        "a member of the project was told what the organization holds: {body}"
    );
}

/// Everyone invited counts against the ceiling, each address once — asked
/// again, or to a second project, it is promised no further seat — and a
/// withdrawn invitation gives its place back.
#[sqlx::test]
async fn everyone_invited_counts_once_against_the_ceiling(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    let project = deployment.project(&pool, &organization, "Payments").await;
    deployment.entitlements.hold(&organization, 3);

    let (status, body) = deployment
        .invite(&pool, &organization, "a@example.test")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = deployment
        .invite_to(&pool, &organization, &project, "b@example.test")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let b = body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        deployment
            .ceiling_read(&pool, &organization, "/internal/v1/organization/members")
            .await["held"],
        3,
        "the owner and two invited"
    );

    let (status, body) = deployment
        .invite(&pool, &organization, "c@example.test")
        .await;
    assert_past_the_ceiling(status, &body, 3);
    // The same address again replaces its invitation and promises no further
    // seat, to the organization as to one of its projects.
    let (status, body) = deployment
        .invite(&pool, &organization, "a@example.test")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, body) = deployment
        .invite_to(&pool, &organization, &project, "a@example.test")
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let (status, body) = deployment
        .call(
            &pool,
            "DELETE",
            &format!("/internal/v1/projects/{project}/invites/{b}"),
            OWNER,
            Some(&organization),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = deployment
        .invite(&pool, &organization, "c@example.test")
        .await;
    assert_eq!(status, StatusCode::CREATED, "the withdrawn place: {body}");
}

/// A seat keeps the promise to the ceiling the organization holds when it
/// is taken: an invitation sent while there was room seats nobody once there
/// is none, by the bell or by the link, and stays live; a member joining a
/// second project takes no seat.
#[sqlx::test]
async fn a_seat_is_held_to_the_ceiling_when_it_is_taken(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, ADMIN).await;
    deployment.sign_in(&pool, STRANGER).await;
    let (status, body) = deployment
        .invite(&pool, &organization, &format!("{ADMIN}@example.test"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let admins = body["id"].as_str().unwrap().to_owned();
    let (status, body) = deployment
        .invite(&pool, &organization, &format!("{STRANGER}@example.test"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let strangers = body["id"].as_str().unwrap().to_owned();
    let link = body["link"].as_str().unwrap().to_owned();
    let token = link.rsplit('/').next().unwrap().to_owned();

    deployment.entitlements.hold(&organization, 2);
    let (status, body) = deployment.accept(&pool, ADMIN, &admins).await;
    assert_eq!(status, StatusCode::OK, "the last place: {body}");
    let (status, body) = deployment.accept(&pool, STRANGER, &strangers).await;
    assert_past_the_ceiling(status, &body, 2);
    let (status, body) = deployment
        .call(
            &pool,
            "POST",
            "/internal/v1/invites/accept",
            STRANGER,
            None,
            Some(json!({ "token": token })),
        )
        .await;
    assert_past_the_ceiling(status, &body, 2);
    assert_eq!(seated(&pool, &organization).await, 2);
    assert_eq!(
        live_invitations(&pool).await,
        1,
        "a refused seat spent or lost its invitation"
    );

    // Somebody already on the roster is promised, and takes, a second
    // project's seat with the organization full.
    let project = deployment.project(&pool, &organization, "Payments").await;
    let (status, body) = deployment
        .invite_to(
            &pool,
            &organization,
            &project,
            &format!("{ADMIN}@example.test"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let seat = body["id"].as_str().unwrap().to_owned();
    let (status, body) = deployment.accept(&pool, ADMIN, &seat).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(seated(&pool, &organization).await, 2);
}

/// A paused organization seats nobody either: an invitation sent while it
/// took things in is refused at the accept, in the deployment's words.
#[sqlx::test]
async fn a_paused_organization_seats_nobody(pool: PgPool) {
    apply_audit_migrations(&pool).await;
    let deployment = Deployment::default();
    let organization = deployment.sign_in(&pool, OWNER).await;
    deployment.sign_in(&pool, ADMIN).await;
    let (status, body) = deployment
        .invite(&pool, &organization, &format!("{ADMIN}@example.test"))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let invite = body["id"].as_str().unwrap().to_owned();
    deployment
        .entitlements
        .paused
        .lock()
        .unwrap()
        .insert(organization.clone());

    let (status, body) = deployment.accept(&pool, ADMIN, &invite).await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["detail"], "This organization is paused.");
    assert_eq!(seated(&pool, &organization).await, 1);
}
