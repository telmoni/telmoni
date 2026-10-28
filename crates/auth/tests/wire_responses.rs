//! Every answer the console reads from auth, written from its struct to
//! `contract/responses/auth/` (`make contract`), which the console's schemas
//! parse (`web/lib/server/entities/responses.test.ts`).
#![expect(clippy::expect_used, reason = "test scaffolding")]

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use telmoni_auth::audit_export::{ExportFailure, ExportFormat};
use telmoni_auth::db::audit::AuditRow;
use telmoni_auth::db::audit_exports::{ExportEntry, ExportState};
use telmoni_auth::db::invites::{IncomingInvite, Invite};
use telmoni_auth::db::members::{GivenSeat, Member, Membership, ProjectOffer};
use telmoni_auth::db::organization_members::{
    OrganizationInvite, OrganizationMember, OrganizationMembership,
};
use telmoni_auth::db::projects::{ProjectEverywhere, ProjectSummary};
use telmoni_auth::db::sessions::SessionRow;
use telmoni_auth::handler::account::EmailChanged;
use telmoni_auth::handler::answer::{
    Declined, InviteCreated, InviteRevoked, Invites, Projects, Roster,
};
use telmoni_auth::handler::audit::{AuditPage, ExportList};
use telmoni_auth::handler::deletion::{AccountDeleted, OrganizationDeleted};
use telmoni_auth::handler::handover::OfferAnswer;
use telmoni_auth::handler::invite::{
    InviteLook, RosterCeiling, Seated, SeatedOn, Settled, Settlement,
};
use telmoni_auth::handler::me::{MeResponse, PersonSummary};
use telmoni_auth::handler::member::{SeatGiven, SeatSeen};
use telmoni_auth::handler::organization::{OrganizationCreated, OrganizationNamed};
use telmoni_auth::handler::password::SignInResponse;
use telmoni_auth::handler::project_transfer::{ProjectAccepted, ProjectMoved};
use telmoni_auth::handler::projects::ProjectRenamed;
use telmoni_auth::handler::session::{
    AuthnResult, ExternalSummary, LogoutUrlResponse, SignInConfig, StartResponse,
};
use telmoni_auth::handler::sessions::Sessions;
use telmoni_auth::model::{ApiToken, CreateTokenResponse, InviteScope};
use telmoni_shared::Email;
use telmoni_shared::slug::Slug;
use telmoni_shared::test_util::pin_contract;
use telmoni_shared::{
    ApiKeyScope, FlagSet, OrganizationId, OrganizationRole, ProjectId, Role, UserId,
};

const INVITE: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2f01);
const SEAT: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2f02);
const KEY: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2f03);
const SESSION: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2f04);
const EVENT: Uuid = Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_2f05);
const EMAIL: &str = "sam@example.com";

/// `answer` as `contract/responses/auth/<name>.json` holds it.
fn pin(name: &str, answer: &impl Serialize) {
    let value = serde_json::to_value(answer).expect("an answer serializes");
    pin_contract(&format!("responses/auth/{name}.json"), &value);
}

/// A fixed moment, `secs` after the first.
fn at(secs: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000 + secs, 0).expect("a time")
}

fn organization() -> OrganizationId {
    OrganizationId::try_new("org_9TzQ1wXe5RkN8bVc").expect("an organization id")
}

fn elsewhere() -> OrganizationId {
    OrganizationId::try_new("org_3LmP8sQd2VwK7xNa").expect("an organization id")
}

fn project() -> ProjectId {
    ProjectId::try_new("project_4kRrX0bQhYd2LmZp").expect("a project id")
}

fn person() -> UserId {
    UserId::try_new("user_7Hq2mK9xVb4LpR3s").expect("a user id")
}

fn ceiling() -> RosterCeiling {
    RosterCeiling {
        limit: 3,
        held: 2,
        reached: false,
        detail: "Hobby holds one member; move to Pro for more.".into(),
    }
}

/// The sign-in page's methods.
#[test]
fn the_sign_in_config_answers_as_the_console_reads() {
    pin(
        "config",
        &SignInConfig {
            password_sign_in: false,
            allow_sign_up: true,
            verify_email: true,
            external: Some(ExternalSummary {
                name: "Okta".into(),
            }),
        },
    );
}

/// A sign-in's start: where to send the browser.
#[test]
fn a_sign_in_start_answers_as_the_console_reads() {
    pin(
        "start",
        &StartResponse {
            authorize_url: "https://id.example.com/authorize?client_id=telmoni".into(),
        },
    );
}

/// A sign-in finished, or refreshed: the person and their tokens.
#[test]
fn a_sign_in_answers_as_the_console_reads() {
    pin(
        "authn",
        &AuthnResult {
            user_id: person().into_inner(),
            email: Some(EMAIL.into()),
            email_verified: true,
            first_name: Some("Sam".into()),
            last_name: Some("Rivera".into()),
            access_token: "at_not_a_real_token".into(),
            session_id: "ses_Qw3rTy7uI9oP1aSd".into(),
            refresh_token: Some("rt_not_a_real_token".into()),
            expires_in: 900,
            id_token: Some("not.a.jwt".into()),
            auth_method: Some("magic_auth".into()),
        },
    );
}

/// Where a sign-out sends the browser.
#[test]
fn a_logout_url_answers_as_the_console_reads() {
    pin(
        "logout-url",
        &LogoutUrlResponse {
            logout_url: "https://id.example.com/logout".into(),
        },
    );
}

/// The built-in form's sign-in, as a code to exchange.
#[test]
fn a_password_sign_in_answers_as_the_console_reads() {
    pin(
        "sign-in",
        &SignInResponse {
            code: "code_not_a_real_one".into(),
        },
    );
}

/// The person's live sessions.
#[test]
fn the_sessions_answer_as_the_console_reads() {
    pin(
        "sessions",
        &Sessions {
            sessions: vec![SessionRow {
                id: SESSION,
                user_agent: Some("Mozilla/5.0".into()),
                created_at: at(0),
                last_seen_at: at(60),
            }],
        },
    );
}

/// Who the person is, and everything the console opens on.
#[test]
fn me_answers_as_the_console_reads() {
    pin(
        "me",
        &MeResponse {
            person: PersonSummary {
                user_id: person(),
                email: Email::from_trusted(EMAIL),
                display_name: Some("Sam Rivera".into()),
                analytics_opt_in: true,
                email_verified: true,
            },
            organizations: vec![OrganizationMembership {
                organization_id: organization(),
                slug: Slug::from_trusted("acme"),
                name: "Acme".into(),
                owner_email: Some(Email::from_trusted(EMAIL)),
                owner_display_name: Some("Sam Rivera".into()),
                role: OrganizationRole::Owner,
                ownership_offer_expires_at: Some(at(604_800)),
                created_at: at(0),
            }],
            active_organization_id: Some(organization()),
            default_organization_id: Some(organization()),
            memberships: vec![Membership {
                project_id: project(),
                organization_id: organization(),
                role: Role::Admin,
            }],
            incoming_invites: vec![IncomingInvite {
                id: INVITE,
                scope: InviteScope::Project,
                target_id: project().into_inner(),
                target_name: "Support bot".into(),
                role: "member".into(),
                inviter_email: Some(Email::from_trusted("alex@example.com")),
                inviter_display_name: None,
                expires_at: at(604_800),
                created_at: at(0),
            }],
            given_seats: vec![GivenSeat {
                project_id: project(),
                project_name: "Support bot".into(),
                project_slug: Slug::from_trusted("support-bot"),
                organization_id: organization(),
                organization_name: "Acme".into(),
                organization_slug: Slug::from_trusted("acme"),
                role: Role::Member,
                giver_email: Some(Email::from_trusted("alex@example.com")),
                giver_display_name: Some("Alex".into()),
                created_at: at(0),
            }],
            project_offers: vec![ProjectOffer {
                project_id: project(),
                name: "Support bot".into(),
                organization_id: elsewhere(),
                organization_name: "Elsewhere".into(),
                owner_email: Some(Email::from_trusted("alex@example.com")),
                owner_display_name: None,
                expires_at: at(604_800),
            }],
            flags: FlagSet::all_on(),
            first_login: false,
            session_row_id: SESSION,
        },
    );
}

/// An account's deletion, finished at once.
#[test]
fn an_account_deletion_answers_as_the_console_reads() {
    pin("account-deleted", &AccountDeleted { deleted: true });
}

/// An address changed.
#[test]
fn an_email_change_answers_as_the_console_reads() {
    pin(
        "email-changed",
        &EmailChanged {
            email: Email::from_trusted(EMAIL),
            sessions_revoked: 2,
        },
    );
}

/// A seat seen by its holder.
#[test]
fn a_seat_seen_answers_as_the_console_reads() {
    pin(
        "seat-seen",
        &SeatSeen {
            organization_id: Some(organization()),
        },
    );
}

/// An incoming invitation accepted from the bell.
#[test]
fn a_settled_invitation_answers_as_the_console_reads() {
    pin(
        "settled-invite",
        &Settled {
            status: Settlement::Accepted,
            scope: InviteScope::Project,
            target_id: project().into_inner(),
            invite_id: INVITE,
            inviter_email: Some(Email::from_trusted("alex@example.com")),
            owner_organization_id: organization(),
        },
    );
}

/// An invitation accepted by its link.
#[test]
fn an_invitation_accepted_by_its_link_answers_as_the_console_reads() {
    pin(
        "seated",
        &Seated {
            on: SeatedOn::ProjectId(project()),
            invite_id: INVITE,
            inviter_email: None,
            owner_organization_id: organization(),
        },
    );
}

/// What an invitation's link opens.
#[test]
fn an_invitation_looked_up_answers_as_the_console_reads() {
    pin(
        "invite-look",
        &InviteLook {
            scope: InviteScope::Organization,
            inviter: "Alex".into(),
            organization: "Acme".into(),
            email: Email::from_trusted(EMAIL),
            role: "admin",
        },
    );
}

fn summary() -> ProjectSummary {
    ProjectSummary {
        id: project(),
        slug: Slug::from_trusted("support-bot"),
        name: "Support bot".into(),
        role: Role::Admin,
    }
}

/// The organization's projects, a project made, and one renamed.
#[test]
fn the_projects_answer_as_the_console_reads() {
    pin(
        "projects",
        &Projects {
            projects: vec![summary()],
        },
    );
    pin("project", &summary());
    pin(
        "project-renamed",
        &ProjectRenamed {
            name: "Support bot".into(),
            slug: Slug::from_trusted("support-bot"),
        },
    );
}

/// Every project the person can open, in every organization.
#[test]
fn the_projects_everywhere_answer_as_the_console_reads() {
    pin(
        "projects-everywhere",
        &Projects {
            projects: vec![ProjectEverywhere {
                id: project(),
                slug: Slug::from_trusted("support-bot"),
                name: "Support bot".into(),
                role: Role::Member,
                organization_id: organization(),
                organization_slug: Slug::from_trusted("acme"),
                organization_name: "Acme".into(),
            }],
        },
    );
}

/// A project's roster, its ceiling, and its outstanding invitations.
#[test]
fn a_project_roster_answers_as_the_console_reads() {
    pin(
        "members",
        &Roster {
            members: vec![Member {
                id: SEAT,
                member_id: person(),
                email: Email::from_trusted(EMAIL),
                display_name: Some("Sam Rivera".into()),
                role: Role::Admin,
                created_at: at(0),
                offer_expires_at: Some(at(604_800)),
            }],
            ceiling: Some(ceiling()),
        },
    );
    pin(
        "invites",
        &Invites {
            invites: vec![Invite {
                id: INVITE,
                email: Email::from_trusted("alex@example.com"),
                role: Role::Member,
                expires_at: at(604_800),
                created_at: at(0),
            }],
        },
    );
}

/// A seat given from the roster.
#[test]
fn a_seat_given_answers_as_the_console_reads() {
    pin(
        "seat-given",
        &SeatGiven {
            id: SEAT,
            member_id: person(),
            email: Email::from_trusted(EMAIL),
            role: Role::Member,
            owner_organization_id: organization(),
        },
    );
}

/// An invitation made and one withdrawn, to either roster.
#[test]
fn an_invitation_made_or_withdrawn_answers_as_the_console_reads() {
    pin(
        "invite-created",
        &InviteCreated {
            id: INVITE,
            link: "https://telmoni.com/invite/not_a_real_secret".into(),
            expires_at: at(604_800),
            owner_organization_id: organization(),
        },
    );
    pin(
        "invite-revoked",
        &InviteRevoked {
            invite_id: INVITE,
            email: Email::from_trusted("alex@example.com"),
            owner_organization_id: organization(),
        },
    );
}

/// A handover offered, of the organization or of a project.
#[test]
fn an_offer_answers_as_the_console_reads() {
    pin(
        "offer",
        &OfferAnswer {
            offered_to: person(),
            offered_to_email: Some(Email::from_trusted(EMAIL)),
            withdrawn_from_email: Some(Email::from_trusted("alex@example.com")),
            expires_at: Some(at(604_800)),
            owner_organization_id: organization(),
        },
    );
}

/// A project taken over, and a handover declined.
#[test]
fn a_project_handover_answers_as_the_console_reads() {
    pin(
        "project-accepted",
        &ProjectAccepted {
            moved: ProjectMoved {
                project_id: project(),
                slug: Slug::from_trusted("support-bot"),
                organization_id: elsewhere(),
                organization_slug: Some(Slug::from_trusted("elsewhere")),
                previous_organization_id: organization(),
                name: "Support bot".into(),
            },
            previous_owner: person(),
        },
    );
    pin(
        "declined",
        &Declined {
            owner_organization_id: organization(),
        },
    );
}

/// An organization founded, renamed and deleted.
#[test]
fn an_organization_answers_as_the_console_reads() {
    pin(
        "organization-created",
        &OrganizationCreated {
            id: organization(),
            slug: Slug::from_trusted("acme"),
            name: "Acme".into(),
        },
    );
    pin(
        "organization-named",
        &OrganizationNamed {
            name: "Acme".into(),
            slug: Slug::from_trusted("acme"),
        },
    );
    pin(
        "organization-deleted",
        &OrganizationDeleted {
            purged: true,
            erase_after: at(900),
        },
    );
}

/// The organization's roster and its outstanding invitations.
#[test]
fn the_organization_roster_answers_as_the_console_reads() {
    pin(
        "organization-members",
        &Roster {
            members: vec![OrganizationMember {
                id: SEAT,
                member_id: person(),
                email: Email::from_trusted(EMAIL),
                display_name: None,
                role: OrganizationRole::Admin,
                created_at: at(0),
                offer_expires_at: None,
            }],
            ceiling: None,
        },
    );
    pin(
        "organization-invites",
        &Invites {
            invites: vec![OrganizationInvite {
                id: INVITE,
                email: Email::from_trusted("alex@example.com"),
                role: OrganizationRole::Member,
                expires_at: at(604_800),
                created_at: at(0),
            }],
        },
    );
}

/// The project's keys, and one minted.
#[test]
fn the_keys_answer_as_the_console_reads() {
    pin(
        "tokens",
        &[ApiToken {
            id: KEY,
            project_id: project().into_inner(),
            name: "ci".into(),
            description: Some("the deploy pipeline".into()),
            token_hint: "tm_Ab3…x9Qz".into(),
            created_by: person().into_inner(),
            scopes: vec![ApiKeyScope::Read, ApiKeyScope::Ingest],
            expires_at: Some(at(7_776_000)),
            last_used_at: Some(at(60)),
            revoked_at: None,
            created_at: at(0),
        }],
    );
    pin(
        "token-minted",
        &CreateTokenResponse {
            id: KEY,
            name: "ci".into(),
            token: "tm_not_a_real_key".into(),
        },
    );
}

/// A page of the audit log.
#[test]
fn the_audit_log_answers_as_the_console_reads() {
    pin(
        "audit",
        &AuditPage {
            events: vec![AuditRow {
                id: EVENT,
                organization_id: organization(),
                actor_id: person().into_inner(),
                action: "created".into(),
                resource_kind: "token".into(),
                resource_id: Some(KEY.to_string()),
                request_id: Some("req_1".into()),
                metadata: Some(serde_json::json!({ "name": "ci" })),
                created_at: at(0),
                prev_hash: Some("0".repeat(64)),
                row_hash: Some("f".repeat(64)),
                in_project: Some(project()),
            }],
            next_cursor: Some(EVENT),
        },
    );
}

/// The person's audit exports, one in each state, and one asked for.
#[test]
fn the_audit_exports_answer_as_the_console_reads() {
    let entry = |n: u128, state| ExportEntry {
        id: Uuid::from_u128(0x0192_f3a1_7c2e_7b4d_9a61_3c5e_8f0d_3000 + n),
        format: ExportFormat::Csv,
        range_from: Some(at(0)),
        range_to: at(86_400),
        created_at: at(90_000),
        state,
    };
    pin(
        "audit-exports",
        &ExportList {
            exports: vec![
                entry(1, ExportState::Queued),
                entry(2, ExportState::Running),
                entry(
                    3,
                    ExportState::Ready {
                        row_count: 120,
                        bytes: 48_000,
                        finished_at: at(90_060),
                        expires_at: at(694_860),
                        downloaded_at: None,
                    },
                ),
                entry(
                    4,
                    ExportState::Failed {
                        failure: ExportFailure::TooLarge,
                        finished_at: at(90_060),
                        expires_at: at(694_860),
                    },
                ),
            ],
            max_rows: 25_000,
            max_bytes: 5_000_000,
        },
    );
    pin("audit-export", &entry(5, ExportState::Queued));
}
