//! What the agent may read and what it forgets, against the real policies:
//! the handler's pool runs as the `agent` role with the production grants,
//! so row-level security applies, and every fixture is seeded as the owner.
//!
//! The embedder is a double whose every vector is the same, so each search
//! matches every passage by meaning and by the shared word, and what comes
//! back is exactly what the scope and the role let through.
#![expect(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "test scaffolding"
)]

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::http::HeaderMap;
use sqlx::PgPool;

use chrono::{DateTime, Utc};
use telmoni_agent::config::{Config, EMBEDDING_DIMENSIONS, EmbeddingsConfig};
use telmoni_agent::db::{self, AgentLane, Source, Visibility};
use telmoni_agent::embed::Embedder;
use telmoni_agent::index::{self, Entry, Passage};
use telmoni_agent::seam::AgentSeam;
use telmoni_agent::{AppState, retention, retrieve};
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Agent, Auth, Emitted, Notice, Notifications, ProjectHome};
use telmoni_shared::test_util::service_pool;
use telmoni_shared::{
    AuthError, FlagSet, OrganizationId, OrganizationRole, OrganizationStatus, ProjectId, Role,
    TelmoniError, UserId,
};

/// Every text is the same unit vector, and each call is counted.
struct SameVector(AtomicUsize);

#[async_trait]
impl Embedder for SameVector {
    fn model(&self) -> &str {
        "same-vector"
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, TelmoniError> {
        self.0.fetch_add(texts.len(), Ordering::SeqCst);
        Ok(texts.iter().map(|_| unit()).collect())
    }
}

fn unit() -> Vec<f32> {
    let mut v = vec![0.0; EMBEDDING_DIMENSIONS];
    v[0] = 1.0;
    v
}

/// Auth and notifications, which nothing here reaches.
struct Absent;

#[async_trait]
impl Auth for Absent {
    async fn resolve(&self, _: &HeaderMap) -> Result<Acting, TelmoniError> {
        Err(AuthError::Unauthenticated.into())
    }
    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        Err(AuthError::IdentityUnavailable.into())
    }
    async fn organization_standing(
        &self,
        _: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        Ok(None)
    }
}

#[async_trait]
impl Notifications for Absent {
    async fn emit(
        &self,
        _: &OrganizationId,
        _: Option<&ProjectId>,
        _: Notice<'_>,
    ) -> Result<Emitted, TelmoniError> {
        Err(TelmoniError::Internal("not here".into()))
    }
    async fn purge_organization(&self, _: &OrganizationId) -> Result<u64, TelmoniError> {
        Ok(0)
    }
    async fn purge_project(&self, _: &ProjectId) -> Result<u64, TelmoniError> {
        Ok(0)
    }
    async fn redact_person(&self, _: &UserId) -> Result<u64, TelmoniError> {
        Ok(0)
    }
}

fn state(pool: &PgPool, embedder: Arc<SameVector>) -> AppState {
    state_with(pool, embedder, Arc::new(Absent))
}

fn state_with(pool: &PgPool, embedder: Arc<SameVector>, auth: Arc<dyn Auth>) -> AppState {
    AppState {
        db: service_pool(pool, "agent"),
        config: Config {
            database_url: String::new(),
            model: None,
            embeddings: EmbeddingsConfig {
                url: "http://embeddings.invalid/v1".into(),
                model: "same-vector".into(),
                api_key: None,
                request_dimensions: false,
            },
            rerank: None,
            docs_corpus_url: None,
            messages_per_hour: 60,
        },
        service_secrets: ServiceSecrets::new("secret", None::<String>),
        auth,
        notifications: Arc::new(Absent),
        model: None,
        embedder: Some(embedder),
        reranker: None,
        observer: None,
    }
}

fn acting(user: &str, role: Role, organization_role: Option<OrganizationRole>) -> Acting {
    Acting {
        user_id: UserId::try_new(user).unwrap(),
        organization_id: OrganizationId::try_new("org_a").unwrap(),
        organization_role,
        project: Some(ActingProject {
            project_id: ProjectId::try_new("proj_a").unwrap(),
            role,
        }),
        session_id: None,
        expires_at: 0,
    }
}

/// One passage, seeded as the owner: `(title, organization, project, user,
/// subject, source, visibility)`.
type Seed<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
    &'a str,
);

async fn seed_chunks(pool: &PgPool, seeds: &[Seed<'_>]) {
    let vector = telmoni_agent::embed::literal(&unit()).unwrap();
    for (title, organization, project, user, subject, source, visibility) in seeds {
        sqlx::query(
            "INSERT INTO agent.chunks (organization_id, project_id, user_id, subject_user_id,
                 source, source_id, visibility, title, body, content_hash, embedding, model,
                 source_created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $6, 'canary passage', 'h', $8::vector,
                 'same-vector', now())",
        )
        .bind(organization)
        .bind(project)
        .bind(user)
        .bind(subject)
        .bind(source)
        .bind(title)
        .bind(visibility)
        .bind(&vector)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// The whole cast: what each person may and may not find.
const CAST: &[Seed<'static>] = &[
    ("docs", None, None, None, None, "docs", "everyone"),
    (
        "our delivery",
        Some("org_a"),
        Some("proj_a"),
        None,
        None,
        "delivery",
        "everyone",
    ),
    (
        "their delivery",
        Some("org_b"),
        Some("proj_b"),
        None,
        None,
        "delivery",
        "everyone",
    ),
    (
        "our sibling project",
        Some("org_a"),
        Some("proj_a2"),
        None,
        None,
        "feed",
        "everyone",
    ),
    (
        "our project audit",
        Some("org_a"),
        Some("proj_a"),
        None,
        None,
        "audit",
        "audit",
    ),
    (
        "our organization audit",
        Some("org_a"),
        None,
        None,
        None,
        "audit",
        "organization_admin",
    ),
    (
        "their organization audit",
        Some("org_b"),
        None,
        None,
        None,
        "audit",
        "organization_admin",
    ),
    (
        "my question",
        Some("org_a"),
        Some("proj_a"),
        Some("user_1"),
        None,
        "conversation",
        "author",
    ),
    (
        "their question",
        Some("org_a"),
        Some("proj_a"),
        Some("user_2"),
        None,
        "conversation",
        "author",
    ),
    (
        "my question elsewhere",
        Some("org_b"),
        Some("proj_b"),
        Some("user_1"),
        None,
        "conversation",
        "author",
    ),
    (
        "notice naming me",
        Some("org_a"),
        Some("proj_a"),
        None,
        Some("user_1"),
        "feed",
        "everyone",
    ),
    (
        "my question in our sibling project",
        Some("org_a"),
        Some("proj_a2"),
        Some("user_1"),
        None,
        "conversation",
        "author",
    ),
    (
        "their audit of this project before it moved",
        Some("org_b"),
        Some("proj_a"),
        None,
        None,
        "audit",
        "audit",
    ),
];

/// What a person finds — by terms and meaning together, and by meaning
/// alone: a query no passage holds a word of leaves the search to its
/// vector half, which must find the same rows, neither more nor fewer.
async fn found(state: &AppState, acting: &Acting) -> BTreeSet<String> {
    let search = |query: &'static str| async move {
        retrieve::search(state, acting, query, 50)
            .await
            .unwrap()
            .into_iter()
            .map(|f| f.title)
            .collect::<BTreeSet<String>>()
    };
    let both = search("canary").await;
    assert_eq!(
        search("zzqx").await,
        both,
        "the vector half alone finds other rows"
    );
    both
}

fn titles(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

/// Another organization's passages, another project's, and another
/// person's questions never come back; a Member reads no audit event, a
/// project admin the project's, and the organization's owner and admins its
/// own chain.
#[sqlx::test]
async fn a_search_returns_what_the_askers_scope_and_role_read(pool: PgPool) {
    seed_chunks(&pool, CAST).await;
    let state = state(&pool, Arc::new(SameVector(AtomicUsize::new(0))));
    let everyone = ["docs", "our delivery", "my question", "notice naming me"];

    let member = acting("user_1", Role::Member, Some(OrganizationRole::Member));
    assert_eq!(found(&state, &member).await, titles(&everyone));

    let project_admin = acting("user_1", Role::Admin, Some(OrganizationRole::Member));
    let mut expected = everyone.to_vec();
    expected.push("our project audit");
    assert_eq!(found(&state, &project_admin).await, titles(&expected));

    let admin = acting("user_1", Role::Admin, Some(OrganizationRole::Admin));
    expected.push("our organization audit");
    assert_eq!(found(&state, &admin).await, titles(&expected));

    let owner = acting("user_1", Role::Owner, Some(OrganizationRole::Owner));
    assert_eq!(found(&state, &owner).await, titles(&expected));
}

async fn seed_conversation(pool: &PgPool, user: &str, project: &str) -> uuid::Uuid {
    let id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO agent.conversations (organization_id, project_id, user_id, title)
         VALUES ('org_a', $1, $2, 'why') RETURNING id",
    )
    .bind(project)
    .bind(user)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent.messages (conversation_id, organization_id, project_id, user_id,
             role, content)
         VALUES ($1, 'org_a', $2, $3, 'user', 'why')",
    )
    .bind(id)
    .bind(project)
    .bind(user)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// A person's conversations are theirs: another member of the same
/// organization and project neither lists nor opens them.
#[sqlx::test]
async fn another_persons_conversations_do_not_exist(pool: PgPool) {
    let theirs = seed_conversation(&pool, "user_2", "proj_a").await;
    let state = state(&pool, Arc::new(SameVector(AtomicUsize::new(0))));
    let me = UserId::try_new("user_1").unwrap();
    let organization = OrganizationId::try_new("org_a").unwrap();
    let project = ProjectId::try_new("proj_a").unwrap();

    let tx = person_scope(&state.db, &me).await.unwrap();
    let mut tx = tx.bind_organization(&organization).await.unwrap();
    assert!(
        db::list_conversations(&mut tx, &me, &project, 50)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        db::conversation(&mut tx, &me, &project, theirs)
            .await
            .unwrap()
            .is_none()
    );
    assert!(db::messages(&mut tx, theirs, 50).await.unwrap().is_empty());
    let (asked, _) = db::recent_questions(&mut tx, &me).await.unwrap();
    assert_eq!(asked, 0, "their questions count against my cap");
    assert!(!db::delete_conversation(&mut tx, &me, theirs).await.unwrap());
    tx.rollback().await.unwrap();

    let them = UserId::try_new("user_2").unwrap();
    let tx = person_scope(&state.db, &them).await.unwrap();
    let mut tx = tx.bind_organization(&organization).await.unwrap();
    assert_eq!(db::messages(&mut tx, theirs, 50).await.unwrap().len(), 1);
    tx.rollback().await.unwrap();
}

async fn count(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

/// A person's erasure takes their conversations and every passage that is
/// theirs or names them; an organization's purge takes what was held of it.
#[sqlx::test]
async fn erasure_and_the_organizations_purge_clear_the_agents_rows(pool: PgPool) {
    seed_chunks(&pool, CAST).await;
    seed_conversation(&pool, "user_1", "proj_a").await;
    let others = seed_conversation(&pool, "user_2", "proj_a2").await;
    // Someone else's answer that quoted them from the member list, citing a
    // notice that named them.
    sqlx::query(
        "INSERT INTO agent.messages (conversation_id, organization_id, project_id, user_id, role,
             content, citations)
         VALUES ($1, 'org_a', 'proj_a2', 'user_2', 'assistant',
                 'ADA LOVELACE (one@example.com) is an admin; the Ada Lovelaces adapter is not',
                 '[{\"index\": 1, \"title\": \"Ada Lovelace joined the project\", \"url\": \"/proj_a2\"},
                   {\"index\": 2, \"title\": \"one@example.com was invited\", \"url\": null}]')",
    )
    .bind(others)
    .execute(&pool)
    .await
    .unwrap();
    // The same name in an organization the person never reached: somebody
    // else's, and left as it is.
    let elsewhere: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO agent.conversations (organization_id, project_id, user_id, title)
         VALUES ('org_c', 'proj_c', 'user_3', 'Ada Lovelace onboarding') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO agent.messages (conversation_id, organization_id, project_id, user_id,
             role, content)
         VALUES ($1, 'org_c', 'proj_c', 'user_3', 'assistant', 'Ada Lovelace runs onboarding')",
    )
    .bind(elsewhere)
    .execute(&pool)
    .await
    .unwrap();
    let seam = AgentSeam(Arc::new(state(
        &pool,
        Arc::new(SameVector(AtomicUsize::new(0))),
    )));

    seam.erase_person(
        &UserId::try_new("user_1").unwrap(),
        "one@example.com",
        Some("Ada Lovelace"),
        &[OrganizationId::try_new("org_a").unwrap()],
    )
    .await
    .unwrap();
    // Their question elsewhere put them in org_b, which auth never named: the
    // scrub found it, a second round fenced and scrubbed it, and every fence
    // is up again.
    let reached: Vec<(String, bool)> = sqlx::query_as(
        "SELECT organization_id, bool_and(COALESCE(scrubbed_at >= fenced_at, false))
           FROM agent.erasures
          WHERE user_id = 'user_1'
          GROUP BY organization_id ORDER BY organization_id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        reached,
        [("org_a".to_owned(), true), ("org_b".to_owned(), true)],
        "where the erasure reached, and whether its fence is up"
    );
    let untouched: (String, String) = sqlx::query_as(
        "SELECT c.title, m.content
           FROM agent.conversations c JOIN agent.messages m ON m.conversation_id = c.id
          WHERE c.organization_id = 'org_c'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        untouched,
        (
            "Ada Lovelace onboarding".to_owned(),
            "Ada Lovelace runs onboarding".to_owned()
        ),
        "a name is scrubbed only where the person could have been quoted"
    );
    let scrubbed: String = sqlx::query_scalar(
        "SELECT content FROM agent.messages WHERE user_id = 'user_2' AND role = 'assistant'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        scrubbed, "a former member (a former member) is an admin; the Ada Lovelaces adapter is not",
        "the name goes, in any case, and only as a whole word"
    );
    let cited: Vec<String> = sqlx::query_scalar(
        "SELECT c->>'title' FROM agent.messages m, jsonb_array_elements(m.citations) c
          WHERE m.user_id = 'user_2' AND m.role = 'assistant'
          ORDER BY (c->>'index')::int",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        cited,
        [
            "a former member joined the project",
            "a former member was invited"
        ],
        "the titles the answer cites keep neither"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE user_id = 'user_1' OR subject_user_id = 'user_1'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.conversations WHERE user_id = 'user_1'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.messages WHERE user_id = 'user_1'"
        )
        .await,
        0,
        "the messages went with their conversation"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE user_id = 'user_2'"
        )
        .await,
        1,
        "someone else's question went with mine"
    );

    seam.purge_organization(&OrganizationId::try_new("org_b").unwrap())
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE organization_id = 'org_b'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE source = 'docs'"
        )
        .await,
        1,
        "the docs belong to nobody's organization"
    );
}

/// The database's clock, which the fences are written in.
async fn clock(pool: &PgPool) -> DateTime<Utc> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Whether an answer saved now in `organization`, by a turn begun at `since`,
/// is withheld.
async fn fenced(state: &AppState, organization: &str, since: DateTime<Utc>) -> bool {
    let them = UserId::try_new("user_2").unwrap();
    let organization = OrganizationId::try_new(organization).unwrap();
    let tx = person_scope(&state.db, &them).await.unwrap();
    let mut tx = tx.bind_organization(&organization).await.unwrap();
    let fenced = db::erased_during(&mut tx, &organization, since)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    fenced
}

/// ⚠ **An answer still being written when an erasure reaches its
/// organization is withheld.** The turn may have read the person before the
/// scrub took them, and the scrub cannot see it. While a scrub runs, every
/// answer there is withheld; once done, one begun before it still is, and
/// one begun after is not, nor one in an organization it never reached. And
/// the erase auth runs once the memberships are gone, naming no
/// organization, still scrubs the name where the first one reached.
#[sqlx::test]
async fn an_erasure_fences_answers_and_remembers_where_it_reached(pool: PgPool) {
    let others = seed_conversation(&pool, "user_2", "proj_a2").await;
    let state = Arc::new(state(&pool, Arc::new(SameVector(AtomicUsize::new(0)))));
    let seam = AgentSeam(state.clone());
    let me = UserId::try_new("user_1").unwrap();

    // A fence down and its scrub not yet done holds back every answer.
    let mut tx = maintenance_scope(&state.db, AgentLane).await.unwrap();
    let open = db::fence_erasure(&mut tx, &me, &["org_a".to_owned()])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(
        fenced(&state, "org_a", clock(&pool).await).await,
        "an answer begun while the scrub runs"
    );
    let mut tx = maintenance_scope(&state.db, AgentLane).await.unwrap();
    db::close_erasure(&mut tx, &me, &open).await.unwrap();
    tx.commit().await.unwrap();

    let before = clock(&pool).await;
    seam.erase_person(
        &me,
        "one@example.com",
        Some("Ada Lovelace"),
        &[OrganizationId::try_new("org_a").unwrap()],
    )
    .await
    .unwrap();
    let after = clock(&pool).await;
    assert!(
        fenced(&state, "org_a", before).await,
        "an answer begun before"
    );
    assert!(
        !fenced(&state, "org_a", after).await,
        "an answer begun after"
    );
    assert!(
        !fenced(&state, "org_b", before).await,
        "an organization never reached"
    );

    // An answer that landed between the two erases, quoting them by name.
    sqlx::query(
        "INSERT INTO agent.messages (conversation_id, organization_id, project_id, user_id, role, content)
         VALUES ($1, 'org_a', 'proj_a2', 'user_2', 'assistant', 'Ada Lovelace is an admin')",
    )
    .bind(others)
    .execute(&pool)
    .await
    .unwrap();
    seam.erase_person(&me, "one@example.com", Some("Ada Lovelace"), &[])
        .await
        .unwrap();
    let scrubbed: String = sqlx::query_scalar(
        "SELECT content FROM agent.messages WHERE user_id = 'user_2' AND role = 'assistant'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(scrubbed, "a former member is an admin");
}

/// Auth as far as where projects are held: `proj_a` in `org_c` now, `proj_b`
/// in `org_b`, and no `proj_gone` at all.
struct Homes;

#[async_trait]
impl Auth for Homes {
    async fn resolve(&self, _: &HeaderMap) -> Result<Acting, TelmoniError> {
        Err(AuthError::Unauthenticated.into())
    }
    async fn global_flags(&self) -> Result<FlagSet, TelmoniError> {
        Err(AuthError::IdentityUnavailable.into())
    }
    async fn organization_standing(
        &self,
        _: &OrganizationId,
    ) -> Result<Option<OrganizationStatus>, TelmoniError> {
        Ok(None)
    }
    async fn project_homes(
        &self,
        projects: &[ProjectId],
    ) -> Result<Vec<ProjectHome>, TelmoniError> {
        Ok([("proj_a", "org_c"), ("proj_b", "org_b")]
            .into_iter()
            .filter(|(project, _)| projects.iter().any(|p| p.as_str() == *project))
            .map(|(project, organization)| ProjectHome {
                project_id: ProjectId::try_new(project).unwrap(),
                slug: project.replace('_', "-"),
                organization_id: OrganizationId::try_new(organization).unwrap(),
                organization_slug: organization.replace('_', "-"),
            })
            .collect())
    }
}

/// ⚠ **What a project left under an organization that no longer has it goes
/// with the hourly sweep** — moved to another, or deleted: its passages and
/// its conversations there, a remembered exchange that landed late among
/// them. Nothing calls the agent when a project leaves, so nothing a
/// transfer's request did could keep it. What is held where the project is
/// now stays, the organization's own stays, and so does everything while
/// auth cannot say where projects are.
#[sqlx::test]
async fn the_sweep_removes_what_a_project_left_behind(pool: PgPool) {
    seed_chunks(
        &pool,
        &[
            (
                "where it was",
                Some("org_a"),
                Some("proj_a"),
                None,
                None,
                "feed",
                "everyone",
            ),
            (
                "remembered there",
                Some("org_a"),
                Some("proj_a"),
                Some("user_1"),
                None,
                "conversation",
                "author",
            ),
            (
                "where it is",
                Some("org_c"),
                Some("proj_a"),
                None,
                None,
                "feed",
                "everyone",
            ),
            (
                "deleted with it",
                Some("org_b"),
                Some("proj_gone"),
                None,
                None,
                "feed",
                "everyone",
            ),
            (
                "still at home",
                Some("org_b"),
                Some("proj_b"),
                None,
                None,
                "feed",
                "everyone",
            ),
            (
                "the organization's own",
                Some("org_a"),
                None,
                None,
                None,
                "audit",
                "organization_admin",
            ),
        ],
    )
    .await;
    seed_conversation(&pool, "user_1", "proj_a").await;
    let embedder = Arc::new(SameVector(AtomicUsize::new(0)));

    retention::sweep_once(&state(&pool, embedder.clone()))
        .await
        .unwrap();
    assert_eq!(
        count(&pool, "SELECT count(*) FROM agent.chunks").await,
        6,
        "rows went while auth could not say where their projects are"
    );

    retention::sweep_once(&state_with(&pool, embedder, Arc::new(Homes)))
        .await
        .unwrap();
    let left: BTreeSet<String> = sqlx::query_scalar("SELECT title FROM agent.chunks")
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        left,
        titles(&["where it is", "still at home", "the organization's own"])
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM agent.conversations").await,
        0,
        "the conversation asked where the project was went too"
    );
}

fn entry(body: &str) -> Entry {
    Entry {
        source_id: "row_1".into(),
        organization_id: Some(OrganizationId::try_new("org_a").unwrap()),
        project_id: Some(ProjectId::try_new("proj_a").unwrap()),
        user_id: None,
        subject_user_id: None,
        visibility: Visibility::Everyone,
        created_at: chrono::Utc::now(),
        passages: vec![
            Passage {
                title: "t".into(),
                body: body.into(),
                url: Some("/proj_a".into()),
            },
            Passage {
                title: "t2".into(),
                body: "second".into(),
                url: None,
            },
        ],
    }
}

/// A row read again with nothing changed costs no embedding; one whose text
/// changed re-embeds that passage alone, and one that shrank loses the rest.
#[sqlx::test]
async fn the_index_embeds_only_what_changed(pool: PgPool) {
    let embedder = Arc::new(SameVector(AtomicUsize::new(0)));
    let state = state(&pool, embedder.clone());
    let write = |entries: Vec<Entry>| {
        let state = &state;
        async move {
            let prepared =
                index::prepare(&state.db, state.embedder().unwrap(), Source::Feed, &entries)
                    .await
                    .unwrap();
            let mut tx = maintenance_scope(&state.db, AgentLane).await.unwrap();
            index::store(&mut tx, Source::Feed, &entries, &prepared)
                .await
                .unwrap();
            tx.commit().await.unwrap();
        }
    };

    write(vec![entry("first")]).await;
    assert_eq!(embedder.0.load(Ordering::SeqCst), 2);
    write(vec![entry("first")]).await;
    assert_eq!(
        embedder.0.load(Ordering::SeqCst),
        2,
        "an unchanged row was embedded again"
    );
    write(vec![entry("changed")]).await;
    assert_eq!(embedder.0.load(Ordering::SeqCst), 3);

    let mut shrunk = entry("changed");
    shrunk.passages.truncate(1);
    write(vec![shrunk]).await;
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE source_id = 'row_1'"
        )
        .await,
        1
    );
}
