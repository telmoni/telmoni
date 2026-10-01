-- The per-module login roles of the LOCAL docker Postgres, created once when
-- its volume is first initialised: docker-compose.yml mounts this under
-- /docker-entrypoint-initdb.d ahead of role_hardening.sql, which needs the
-- four to exist. A tier creates the same roles, each with a secret of its
-- own, before it applies that hardening; here the passwords are the ones
-- .env.example carries, and the instance listens on 127.0.0.1 alone.
--
-- Nothing else logs in as a module: the maintenance lanes are NOLOGIN and the
-- hardening creates them, and the tests connect as the docker superuser
-- (DATABASE_URL) and SET ROLE into these. An edit here lands with
-- `make db-reset`, which recreates the volume.
CREATE ROLE auth          LOGIN PASSWORD 'auth_dev';
CREATE ROLE notifications LOGIN PASSWORD 'notifications_dev';
CREATE ROLE agent         LOGIN PASSWORD 'agent_dev';
CREATE ROLE migrator      LOGIN PASSWORD 'migrator_dev';
