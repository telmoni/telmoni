CREATE SCHEMA IF NOT EXISTS agent;

-- The `migrator` role cannot create an extension; the database's own
-- initialisation does (compose's init script locally, the bootstrap SQL on
-- Cloud SQL). Refused here rather than at the first CREATE TABLE, whose
-- error says only that `vector` is not a type. Before 0.8 there is no
-- `hnsw.iterative_scan`, which a small organization's search in a large
-- index needs, and every search would fail on setting it.
DO $$
DECLARE
    installed TEXT;
BEGIN
    SELECT extversion INTO installed FROM pg_extension WHERE extname = 'vector';
    IF installed IS NULL THEN
        RAISE EXCEPTION 'the vector extension is not installed in this database; a superuser runs CREATE EXTENSION vector before migrating';
    END IF;
    IF string_to_array(installed, '.')::int[] < ARRAY[0, 8] THEN
        RAISE EXCEPTION 'the vector extension is version %, and the agent needs 0.8 or later; a superuser runs ALTER EXTENSION vector UPDATE once a newer pgvector is installed', installed;
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
-- `organization_admin` for the organization's chain and feed (its owner and
-- admins), `author` for a person's own.
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
    CONSTRAINT chunks_visibility_check CHECK (visibility IN ('everyone', 'audit', 'organization_admin', 'author')),
    CONSTRAINT chunks_tenant_check CHECK (
        (source = 'docs') = (organization_id IS NULL)
        AND (source <> 'docs' OR (project_id IS NULL AND user_id IS NULL))
    ),
    CONSTRAINT chunks_author_check CHECK ((visibility = 'author') = (user_id IS NOT NULL)),
    CONSTRAINT chunks_part_check CHECK (part >= 0),
    CONSTRAINT chunks_source_key UNIQUE (source, source_id, part)
);

-- Two graphs, not one: the docs apart from every tenant's rows, so a search
-- reads the docs nearest a question whoever else's rows lie nearer.
CREATE INDEX chunks_docs_embedding_idx   ON agent.chunks USING hnsw (embedding vector_cosine_ops) WHERE organization_id IS NULL;
CREATE INDEX chunks_tenant_embedding_idx ON agent.chunks USING hnsw (embedding vector_cosine_ops) WHERE organization_id IS NOT NULL;
CREATE INDEX chunks_tsv_idx          ON agent.chunks USING gin (tsv);
-- An organization's rows in id order: its purge, and an erasure's scrub of
-- a name there, a batch at a time.
CREATE INDEX chunks_tenant_idx       ON agent.chunks (organization_id, id);
-- The pairs of an organization and a project rows are held under, read one
-- probe a pair without stepping through an organization's own rows; and the
-- rows of one pair, for the sweep that removes what a project left behind.
CREATE INDEX chunks_home_idx         ON agent.chunks (organization_id, project_id) WHERE project_id IS NOT NULL;
-- What a project and its organization share, by who may read it, newest
-- first, without anyone's own past exchanges among it; and a person's own
-- exchanges, by where they asked. The search counts and compares exactly
-- what one person reads through these two alone, reading no row anyone else
-- may — and a larger tenant's newest rows from the index alone, as it
-- carries every column the policies and the order read.
CREATE INDEX chunks_shared_idx       ON agent.chunks (organization_id, project_id, visibility, source_created_at DESC) INCLUDE (user_id, id) WHERE user_id IS NULL;
CREATE INDEX chunks_user_idx         ON agent.chunks (user_id, organization_id, project_id) WHERE user_id IS NOT NULL;
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
-- is its author's, not the project's. The organization is compared as well
-- as the project: a project transferred to another organization keeps its
-- id, and a row its old organization's audit chain wrote about it, indexed
-- before the retention sweep removes what the project left there, must not
-- reach the new one.
CREATE POLICY tenant_isolation ON agent.chunks
    USING      (user_id IS NULL AND project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true))
    WITH CHECK (user_id IS NULL AND project_id = current_setting('app.project_id', true)
                AND organization_id = current_setting('app.organization_id', true));

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
-- were asked from is recorded, for the list and for the sweep that removes
-- what a project left under an organization that no longer has it.
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
-- The organization and project together: the retention sweep finding what a
-- transfer or a delete left under the organization a project no longer
-- belongs to.
CREATE INDEX conversations_home_idx ON agent.conversations (organization_id, project_id);
-- An organization's conversations in id order: its purge, and an erasure's
-- scrub of a name there, a batch at a time.
CREATE INDEX conversations_organization_idx ON agent.conversations (organization_id, id);
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
-- An erased person's name, struck from the titles of other people's.
GRANT UPDATE (title) ON agent.conversations TO agent_maintenance;


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
-- An organization's messages in id order: an erasure's scrub of a name
-- there, a batch at a time.
CREATE INDEX messages_organization_idx ON agent.messages (organization_id, id);
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

-- No DELETE: every lane that removes messages removes their conversation, and
-- they go with it by cascade, which needs none. The text and the titles it
-- cites alone, read and rewritten: an erased person's name and address,
-- struck from other people's answers.
GRANT SELECT (id, organization_id, content, citations), UPDATE (content, citations)
    ON agent.messages TO agent_maintenance;


-- ══════════════════════════════════════════════════════════════════════════════
-- 4. CURSORS
-- ══════════════════════════════════════════════════════════════════════════════
-- How far the indexer has read each source. One row per source, leased by
-- the replica indexing it (`lease_id` until `leased_until`, put off while it
-- works), so replicas share the loop without embedding a page twice. A lease
-- rather than a held row lock: embedding waits on a model for minutes, and
-- no transaction stays open across it. `after_at` and `after_id` are the last
-- row taken, in the source's own order; `digest` is the docs corpus's hash.
CREATE TABLE agent.cursors (
    source        TEXT        PRIMARY KEY,
    after_at      TIMESTAMPTZ,
    after_id      TEXT,
    digest        TEXT,
    lease_id      UUID,
    leased_until  TIMESTAMPTZ,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
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


-- ══════════════════════════════════════════════════════════════════════════════
-- 5. ERASURES
-- ══════════════════════════════════════════════════════════════════════════════
-- A person's erasure, fenced on each organization it scrubs: a row for each
-- erase there, so two erases of one person running at once — one a request
-- gave up waiting for, and the sweep's — each hold a fence of their own, and
-- neither one's close lifts the other's. A turn still answering there may
-- have read them before the scrub took them: an answer that lands while any
-- fence on its organization is down, or after one came up but from a turn
-- begun before, is withheld, not saved. A fence whose scrub stopped stepping
-- (`fenced_at`, put forward at every step) lapses: an erase whose process
-- went away mid-scrub holds nothing back for long. The rows also keep where
-- the erasure reached, for a retry that runs after the person's memberships
-- are gone. Ids only, and gone thirty days after they were last of use.
CREATE TABLE agent.erasures (
    user_id         TEXT        NOT NULL,
    organization_id TEXT        NOT NULL,
    fence_id        UUID        NOT NULL,
    fenced_at       TIMESTAMPTZ NOT NULL,
    scrubbed_at     TIMESTAMPTZ,
    PRIMARY KEY (user_id, organization_id, fence_id)
);

CREATE INDEX erasures_fence_idx ON agent.erasures (organization_id, scrubbed_at);

ALTER TABLE agent.erasures ENABLE ROW LEVEL SECURITY;
ALTER TABLE agent.erasures FORCE ROW LEVEL SECURITY;

-- Read by every answer saved in the organization, for the fence.
CREATE POLICY tenant_isolation ON agent.erasures
    USING      (organization_id = current_setting('app.organization_id', true))
    WITH CHECK (organization_id = current_setting('app.organization_id', true));

CREATE POLICY maintenance_access ON agent.erasures
    TO agent_maintenance
    USING      (current_user = 'agent_maintenance')
    WITH CHECK (current_user = 'agent_maintenance');

GRANT SELECT, INSERT, UPDATE, DELETE ON agent.erasures TO agent_maintenance;
