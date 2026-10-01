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

use telmoni_agent::config::{Config, EMBEDDING_DIMENSIONS, EmbeddingsConfig};
use telmoni_agent::db::{self, AgentLane, Source, Visibility};
use telmoni_agent::embed::Embedder;
use telmoni_agent::index::{self, Entry, Passage};
use telmoni_agent::seam::AgentSeam;
use telmoni_agent::{AppState, retrieve};
use telmoni_shared::acting::{Acting, ActingProject};
use telmoni_shared::db::tenant_session::{maintenance_scope, person_scope};
use telmoni_shared::middleware::service_auth::ServiceSecrets;
use telmoni_shared::seam::{Agent, Auth, Emitted, Notice, Notifications};
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
        auth: Arc::new(Absent),
        notifications: Arc::new(Absent),
        model: None,
        embedder: Some(embedder),
        reranker: None,
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
        "owner",
    ),
    (
        "their organization audit",
        Some("org_b"),
        None,
        None,
        None,
        "audit",
        "owner",
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
];

async fn found(state: &AppState, acting: &Acting) -> BTreeSet<String> {
    retrieve::search(state, acting, "canary", 50)
        .await
        .unwrap()
        .into_iter()
        .map(|f| f.title)
        .collect()
}

fn titles(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

/// Another organization's passages, another project's, and another
/// person's questions never come back; a Member reads no audit event, an
/// admin the project's, and only the owner the organization's own chain.
#[sqlx::test]
async fn a_search_returns_what_the_askers_scope_and_role_read(pool: PgPool) {
    seed_chunks(&pool, CAST).await;
    let state = state(&pool, Arc::new(SameVector(AtomicUsize::new(0))));
    let everyone = ["docs", "our delivery", "my question", "notice naming me"];

    let member = acting("user_1", Role::Member, Some(OrganizationRole::Member));
    assert_eq!(found(&state, &member).await, titles(&everyone));

    let admin = acting("user_1", Role::Admin, Some(OrganizationRole::Admin));
    let mut expected = everyone.to_vec();
    expected.push("our project audit");
    assert_eq!(found(&state, &admin).await, titles(&expected));

    let owner = acting("user_1", Role::Owner, Some(OrganizationRole::Owner));
    expected.push("our organization audit");
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
        db::conversation(&mut tx, &me, theirs)
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
/// theirs or names them; a project's purge takes what was held of it.
#[sqlx::test]
async fn erasure_and_the_project_purge_clear_the_agents_rows(pool: PgPool) {
    seed_chunks(&pool, CAST).await;
    seed_conversation(&pool, "user_1", "proj_a").await;
    seed_conversation(&pool, "user_2", "proj_a2").await;
    let seam = AgentSeam(Arc::new(state(
        &pool,
        Arc::new(SameVector(AtomicUsize::new(0))),
    )));

    seam.erase_person(&UserId::try_new("user_1").unwrap())
        .await
        .unwrap();
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

    seam.purge_project(&ProjectId::try_new("proj_a").unwrap())
        .await
        .unwrap();
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE project_id = 'proj_a'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.chunks WHERE project_id = 'proj_a2'"
        )
        .await,
        1,
        "a sibling project's passage went with the purged one"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT count(*) FROM agent.conversations WHERE project_id = 'proj_a2'"
        )
        .await,
        1
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
            let mut tx = maintenance_scope(&state.db, AgentLane).await.unwrap();
            index::write(&mut tx, state.embedder().unwrap(), Source::Feed, &entries)
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
