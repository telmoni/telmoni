-- The extensions of the LOCAL docker Postgres, installed once when its volume
-- is first initialised: docker-compose.yml mounts this under
-- /docker-entrypoint-initdb.d, where it runs as the superuser in the
-- `telmoni` database. The `migrator` role cannot create an extension, so the
-- agent's migration only checks that `vector` is here; a tier's bootstrap
-- installs it the same way.
CREATE EXTENSION IF NOT EXISTS vector;

-- template1 as well: every database created after this one copies it, and
-- each `#[sqlx::test]` database is one of those.
\connect template1
CREATE EXTENSION IF NOT EXISTS vector;
