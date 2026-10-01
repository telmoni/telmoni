CREATE SCHEMA IF NOT EXISTS agent;

-- The `migrator` role cannot create an extension; the database's own
-- initialisation does (compose's init script locally, the bootstrap SQL on
-- Cloud SQL). Refused here rather than at the first CREATE TABLE, whose
-- error says only that `vector` is not a type.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector') THEN
        RAISE EXCEPTION 'the vector extension is not installed in this database; a superuser runs CREATE EXTENSION vector before migrating';
    END IF;
END $$;

-- ══════════════════════════════════════════════════════════════════════════════
-- ROLE BOOTSTRAP & SCHEMA GRANTS
-- ══════════════════════════════════════════════════════════════════════════════
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'agent_maintenance') THEN
        CREATE ROLE agent_maintenance NOLOGIN;
    END IF;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN unique_violation THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'agent_maintenance not created here (no CREATEROLE) — role_hardening.sql owns it in prod';
END $$;

DO $$
BEGIN
    GRANT agent_maintenance TO agent WITH INHERIT FALSE, SET TRUE;
EXCEPTION
    WHEN undefined_object THEN NULL;
    WHEN insufficient_privilege THEN NULL;
END $$;

GRANT USAGE ON SCHEMA agent TO agent_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 1. CHUNKS
-- ══════════════════════════════════════════════════════════════════════════════
-- One searchable passage of a source, keyed the way its source is: the docs
-- with no tenant, an audit event on its organization (and its project when it
-- happened inside one), a feed item or a delivery on its project, a past
-- exchange on its author. `visibility` is the role the source's own lane asks
-- for, which RLS cannot know: `audit` for a project's chain (Read Audit),
-- `owner` for the organization's chain and feed, `author` for a person's own.
CREATE TABLE agent.chunks (
    id                UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id   TEXT,
    project_id        TEXT,
    user_id           TEXT,
    -- The person the text names, so their erasure can drop it.
    subject_user_id   TEXT,
    source            TEXT        NOT NULL,
    source_id         TEXT        NOT NULL,
    part              INTEGER     NOT NULL DEFAULT 0,
    visibility        TEXT        NOT NULL,
    title             TEXT        NOT NULL,
    body              TEXT        NOT NULL,
    url               TEXT,
    content_hash      TEXT        NOT NULL,
    embedding         vector(768) NOT NULL,
    -- The model that embedded it; `agent-reindex` redoes every row whose
    -- model is not the configured one.
    model             TEXT        NOT NULL,
    -- 'simple', not a language: what this half of the search is for is exact
    -- tokens — ids, error codes, connector names — which stemming mangles.
    tsv               tsvector    GENERATED ALWAYS AS (to_tsvector('simple', title || ' ' || body)) STORED,
    source_created_at TIMESTAMPTZ NOT NULL,
    shard_key         UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT chunks_source_check CHECK (source IN ('docs', 'audit', 'feed', 'delivery', 'conversation')),
    CONSTRAINT chunks_visibility_check CHECK (visibility IN ('everyone', 'audit', 'owner', 'author')),
    CONSTRAINT chunks_tenant_check CHECK (
        (source = 'docs') = (organization_id IS NULL)
        AND (source <> 'docs' OR (project_id IS NULL AND user_id IS NULL))
    ),
    CONSTRAINT chunks_author_check CHECK ((visibility = 'author') = (user_id IS NOT NULL)),
    CONSTRAINT chunks_part_check CHECK (part >= 0),
    CONSTRAINT chunks_source_key UNIQUE (source, source_id, part)
);

CREATE INDEX chunks_embedding_idx    ON agent.chunks USING hnsw (embedding vector_cosine_ops);
CREATE INDEX chunks_tsv_idx          ON agent.chunks USING gin (tsv);
CREATE INDEX chunks_tenant_idx       ON agent.chunks (organization_id, project_id);
CREATE INDEX chunks_project_idx      ON agent.chunks (project_id) WHERE project_id IS NOT NULL;
CREATE INDEX chunks_user_idx         ON agent.chunks (user_id) WHERE user_id IS NOT NULL;
CREATE INDEX chunks_subject_user_idx ON agent.chunks (subject_user_id) WHERE subject_user_id IS NOT NULL;
CREATE INDEX chunks_retention_idx    ON agent.chunks (source, source_created_at);
CREATE INDEX chunks_model_idx        ON agent.chunks (model);

ALTER TABLE agent.chunks ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent.chunks FORCE ROW LEVEL SECURITY;

-- The docs: everyone's, and nobody's to write outside the lane.
CREATE POLICY global_read ON agent.chunks
    FOR SELECT
    USING (organization_id IS NULL);

-- `user_id IS NULL` is load-bearing: a past exchange recorded on a project
-- is its author's, not the project's.
CREATE POLICY tenant_isolation ON agent.chunks
    USING      (user_id IS NULL AND project_id = current_setting('app.project_id', true))
    WITH CHECK (user_id IS NULL AND project_id = current_setting('app.project_id', true));

-- `project_id IS NULL` is load-bearing: without it an organization scope reaches every project's rows.
CREATE POLICY organization_level ON agent.chunks
    USING      (user_id IS NULL AND project_id IS NULL AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (user_id IS NULL AND project_id IS NULL AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY author_access ON agent.chunks
    USING      (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON agent.chunks
    TO agent_maintenance
    USING      (current_user = 'agent_maintenance')
    WITH CHECK (current_user = 'agent_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON agent.chunks TO agent_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 2. CONVERSATIONS
-- ══════════════════════════════════════════════════════════════════════════════
-- A person's, inside one organization: `tenant_isolation` compares the person
-- as well, so nobody else in the organization reads them. The project they
-- were asked from is recorded, for the list and for the project's purge.
CREATE TABLE agent.conversations (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    organization_id TEXT        NOT NULL,
    project_id      TEXT        NOT NULL,
    user_id         TEXT        NOT NULL,
    title           TEXT        NOT NULL,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT conversations_title_check CHECK (char_length(title) BETWEEN 1 AND 200),
    CONSTRAINT conversations_author_key UNIQUE (id, user_id, organization_id)
);

CREATE INDEX conversations_author_idx  ON agent.conversations (user_id, organization_id, updated_at DESC);
CREATE INDEX conversations_project_idx ON agent.conversations (project_id);
CREATE INDEX conversations_organization_idx ON agent.conversations (organization_id);
CREATE INDEX conversations_retention_idx ON agent.conversations (updated_at);

ALTER TABLE agent.conversations ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent.conversations FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON agent.conversations
    USING      (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON agent.conversations
    TO agent_maintenance
    USING      (current_user = 'agent_maintenance')
    WITH CHECK (current_user = 'agent_maintenance');

GRANT SELECT, DELETE ON agent.conversations TO agent_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 3. MESSAGES
-- ══════════════════════════════════════════════════════════════════════════════
CREATE TABLE agent.messages (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    conversation_id UUID        NOT NULL,
    organization_id TEXT        NOT NULL,
    project_id      TEXT        NOT NULL,
    user_id         TEXT        NOT NULL,
    role            TEXT        NOT NULL,
    content         TEXT        NOT NULL,
    -- `[{index, title, url}]`, as the reply cited them.
    citations       JSONB       NOT NULL DEFAULT '[]'::jsonb,
    shard_key       UUID        NOT NULL DEFAULT gen_random_uuid(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT messages_role_check CHECK (role IN ('user', 'assistant')),
    CONSTRAINT messages_citations_check CHECK (jsonb_typeof(citations) = 'array'),
    -- The whole author key, not the id alone: a message can only join a
    -- conversation its own person holds in its own organization.
    CONSTRAINT messages_conversation_fkey FOREIGN KEY (conversation_id, user_id, organization_id)
        REFERENCES agent.conversations (id, user_id, organization_id) ON DELETE CASCADE
);

CREATE INDEX messages_conversation_idx ON agent.messages (conversation_id, created_at);
-- The hourly cap counts a person's own questions.
CREATE INDEX messages_rate_idx ON agent.messages (user_id, created_at) WHERE role = 'user';

ALTER TABLE agent.messages ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent.messages FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON agent.messages
    USING      (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (user_id = current_setting('app.user_id', true) AND organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON agent.messages
    TO agent_maintenance
    USING      (current_user = 'agent_maintenance')
    WITH CHECK (current_user = 'agent_maintenance');

-- No grant: every lane that removes messages removes their conversation, and
-- they go with it by cascade, which needs none.


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. CURSORS
-- ══════════════════════════════════════════════════════════════════════════════
-- How far the indexer has read each source. One row per source, locked
-- `FOR UPDATE SKIP LOCKED` by the replica indexing it, so replicas share the
-- loop without reading a page twice. `after_at` and `after_id` are the last
-- row taken, in the source's own order; `digest` is the docs corpus's hash.
CREATE TABLE agent.cursors (
    source     TEXT        PRIMARY KEY,
    after_at   TIMESTAMPTZ,
    after_id   TEXT,
    digest     TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT cursors_source_check CHECK (source IN ('docs', 'audit', 'feed', 'delivery'))
);

INSERT INTO agent.cursors (source) VALUES ('docs'), ('audit'), ('feed'), ('delivery');

ALTER TABLE agent.cursors ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent.cursors FORCE ROW LEVEL SECURITY;

CREATE POLICY maintenance_access ON agent.cursors
    TO agent_maintenance
    USING      (current_user = 'agent_maintenance')
    WITH CHECK (current_user = 'agent_maintenance');

GRANT SELECT, UPDATE ON agent.cursors TO agent_maintenance;
