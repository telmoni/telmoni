# ── Configuration ──────────────────────────────────────────────

.DEFAULT_GOAL := help

ifneq (,$(wildcard .env))
  include .env
endif

export DATABASE_URL ?= postgresql://telmoni:telmoni_dev@localhost:5432/telmoni

WEB_DIR     ?= web
CRATES_DIR  ?= crates
SERVER_PORT ?= 8082
# The console's: scripts/up.sh pins 3000, and a stack built on this one, on
# ports of its own, has port-check probe those instead.
WEB_PORT    ?= 3000
TEST_THREADS ?= 2

MIGRATION_SETS := audit=$(CRATES_DIR)/migrator/migrations,auth=$(CRATES_DIR)/auth/migrations,notifications=$(CRATES_DIR)/notifications/migrations,agent=$(CRATES_DIR)/agent/migrations

export KEY ON WHY ORG BY
FLAG_DATABASE_URL ?= $(MIGRATOR_DATABASE_URL)
PSQL_AUTH = if command -v psql >/dev/null 2>&1; then psql -q -v ON_ERROR_STOP=1 $(FLAG_DATABASE_URL) "$$@"; \
	else docker compose exec -T postgres psql -q -v ON_ERROR_STOP=1 -U telmoni -d telmoni "$$@"; fi

COMMA := ,

# ── Help ───────────────────────────────────────────────────────

.PHONY: help
help:
	@echo "telmoni"
	@echo ""
	@echo "  Start here"
	@echo "    make first-run            fresh-clone bootstrap: env files, install, docker, migrate"
	@echo "    make doctor               audit the dev toolchain (rust/node/docker/...)"
	@echo ""
	@echo "  Develop locally"
	@echo "    make up                   run the whole stack: Postgres, Redis, the server, the console"
	@echo "    make down                 stop docker containers"
	@echo "    make docker-up            run postgres + redis in Docker (headless)"
	@echo "    make server-dev           run the server alone (telmoni serve)"
	@echo "    make web-dev              run the console alone"
	@echo "    make sweep SWEEP=deletion run one sweep by hand (deletion, audit-verify, retention, agent-reindex, audit-exports)"
	@echo "    make webhook-receiver SECRET=whsec_...   receive an outbound webhook behind a named Cloudflare route"
	@echo "    make webhook-tunnel       the same, with no dashboard: a quick tunnel prints a throwaway URL (needs cloudflared)"
	@echo "    make db-migrate           apply every migration set to the local DB"
	@echo "    make db-reset             wipe and rebuild the local DB"
	@echo "    make db-shell             psql into local postgres"
	@echo "    make flag KEY=connectors ON=false WHY=\"...\" BY=<who>   flip a feature flag (ORG=<id> for one organization; make flags lists)"
	@echo ""
	@echo "  Before you push"
	@echo "    make ci                   the full gate — run this (check + lint + test + web + deny + typos)"
	@echo "    make check                the fast half: types, clippy, fmt, rustdoc"
	@echo "    make test                 vitest (web) + cargo test (the crates)"
	@echo "    make fmt                  cargo fmt + prettier"
	@echo ""
	@echo "  More"
	@echo "    make help-all             every target, including the ones the above compose"

.PHONY: help-all
help-all:
	@echo "Every target (make help shows the ~25 you actually type):"
	@$(MAKE) -pRrq 2>/dev/null | grep -E '^[a-z][a-z0-9-]*:' | cut -d: -f1 | sort -u | sed 's/^/  make /'

# ── Setup & Environment ────────────────────────────────────────

.PHONY: install
install:
	cd $(WEB_DIR) && npm ci
	cargo fetch --locked
	@if [ ! -f $(WEB_DIR)/.env.local ]; then \
		echo ""; \
		echo "  web/.env.local not found."; \
		echo "  Create it: cp web/.env.local.example web/.env.local"; \
		echo ""; \
	fi

.PHONY: env-check
env-check: env-check-services env-check-web

.PHONY: env-scaffold
env-scaffold:
	@if [ ! -f .env ]; then \
		cp .env.example .env; \
		echo "· created .env from .env.example"; \
	fi
	@if [ ! -f $(WEB_DIR)/.env.local ]; then \
		cp $(WEB_DIR)/.env.local.example $(WEB_DIR)/.env.local; \
		echo "· created $(WEB_DIR)/.env.local from its example"; \
	fi
	@if grep -qE '^SERVICE_SECRET=[[:space:]]*$$' .env 2>/dev/null || \
	   grep -qE '^SERVICE_SECRET=[[:space:]]*$$' $(WEB_DIR)/.env.local 2>/dev/null; then \
		secret=$$(openssl rand -hex 32 2>/dev/null || head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n'); \
		if [ -z "$$secret" ]; then \
			echo "✗ env-scaffold: could not generate a SERVICE_SECRET (no openssl, no /dev/urandom)."; \
			echo "  Set the SAME value in .env and $(WEB_DIR)/.env.local by hand."; \
			exit 1; \
		fi; \
		for f in .env $(WEB_DIR)/.env.local; do \
			tmp=$$(mktemp) || exit 1; \
			sed "s|^SERVICE_SECRET=[[:space:]]*$$|SERVICE_SECRET=$$secret|" "$$f" > "$$tmp" && mv "$$tmp" "$$f"; \
		done; \
		echo "· minted a SERVICE_SECRET into .env and $(WEB_DIR)/.env.local (same value in both)"; \
	fi

.PHONY: port-check
port-check:
	@if command -v ss >/dev/null 2>&1; then probe() { ss -ltn 2>/dev/null | grep -qE "[:.]$$1[[:space:]]"; }; \
	elif command -v lsof >/dev/null 2>&1; then probe() { lsof -i :$$1 -sTCP:LISTEN >/dev/null 2>&1; }; \
	else echo "· port-check skipped (no ss or lsof)"; exit 0; fi; \
	busy=""; \
	for spec in "$(WEB_PORT) web" "$(SERVER_PORT) server"; do \
		set -- $$spec; \
		if probe $$1; then busy="$$busy $$2:$$1"; fi; \
	done; \
	if [ -n "$$busy" ]; then \
		echo "✗ port(s) already in use:$$busy"; \
		echo "  Something else is listening — often a stale \`make up\` or another"; \
		echo "  project's dev server. Find it, then stop it:"; \
		echo "    ss -ltnp | grep -E ':($(WEB_PORT)|$(SERVER_PORT))\\s'    # Linux"; \
		echo "    lsof -i :$(WEB_PORT) -sTCP:LISTEN                 # macOS"; \
		echo "  If it is a previous \`make up\` for THIS repo, Ctrl-C it there."; \
		exit 1; \
	else \
		echo "✓ ports free — web :$(WEB_PORT), server :$(SERVER_PORT)."; \
	fi

.PHONY: deps-check
deps-check:
	@if [ ! -x $(WEB_DIR)/node_modules/.bin/next ]; then \
		echo "✗ web dependencies are not installed ($(WEB_DIR)/node_modules/.bin/next is missing)."; \
		echo "  Without them \`npm run dev\` dies with \`next: command not found\`, and"; \
		echo "  \`make up\` stops the server in response."; \
		echo "    make first-run     # install + env + db, the whole fresh-clone sequence"; \
		echo "    make install       # just the dependencies"; \
		exit 1; \
	else \
		echo "✓ web dependencies present."; \
	fi

.PHONY: schema-check
schema-check:
	@if ! missing=$$(docker compose exec -T postgres psql -U telmoni -d telmoni -tAc \
		"SELECT coalesce(string_agg(t, ' '), '') FROM unnest(ARRAY['audit.events','auth.organizations','notifications.feed','agent.cursors']) AS t WHERE to_regclass(t) IS NULL" \
		2>/dev/null); then \
		echo "· schema-check skipped (postgres not reachable)"; \
	elif [ -n "$$missing" ]; then \
		echo "✗ database is missing tables: $$missing"; \
		echo "  The server will still start and log \`listening\`, then fail every"; \
		echo "  request with \`relation ... does not exist\`."; \
		echo "    make db-migrate    # apply migrations to the database you have"; \
		echo "    make db-reset      # wipe and rebuild it from scratch"; \
		exit 1; \
	else \
		echo "✓ schema present — audit, auth, notifications, agent."; \
	fi

.PHONY: env-check-services
env-check-services:
	@missing=""; \
	for var in AUTH_DATABASE_URL NOTIFICATIONS_DATABASE_URL MIGRATOR_DATABASE_URL SERVICE_SECRET; do \
		eval val=\$$$$var; \
		if [ -n "$$val" ]; then continue; fi; \
		if [ -f .env ] && grep -qE "^$$var=[^[:space:]]+" .env; then continue; fi; \
		missing="$$missing $$var"; \
	done; \
	if [ -n "$$missing" ]; then \
		echo "Missing server env vars:$$missing"; \
		echo "Set them in .env (start from .env.example, or run \`make env-scaffold\`)"; \
		echo "or export them in your shell."; \
		exit 1; \
	else \
		echo "✓ server env (.env) — all required vars present."; \
	fi

.PHONY: env-check-web
env-check-web:
	@if [ ! -f $(WEB_DIR)/.env.local ]; then \
		echo "✗ $(WEB_DIR)/.env.local not found."; \
		echo "  Create it: cp $(WEB_DIR)/.env.local.example $(WEB_DIR)/.env.local"; \
		exit 1; \
	fi; \
	missing=""; \
	for var in SERVER_URL SERVICE_SECRET AUTH_SECRET AUTH_URL REDIS_URL; do \
		if ! grep -qE "^$$var=[^[:space:]]+" $(WEB_DIR)/.env.local; then \
			missing="$$missing $$var"; \
		fi; \
	done; \
	if [ -n "$$missing" ]; then \
		echo "Missing web env vars in $(WEB_DIR)/.env.local:$$missing"; \
		exit 1; \
	else \
		echo "✓ web env ($(WEB_DIR)/.env.local) — all required vars present."; \
	fi

# ── Development ────────────────────────────────────────────────

.PHONY: up-preflight
up-preflight: env-scaffold env-check port-check deps-check docker-up db-wait roles-check schema-check

.PHONY: up
up: up-preflight
	@echo "building the server (so it boots alongside web, not a minute after)…"
	cargo build -p telmoni --quiet
	SERVER_PORT=$(SERVER_PORT) bash scripts/up.sh

.PHONY: local
local: up

.PHONY: first-run
first-run: env-scaffold
	@$(MAKE) first-run-steps

.PHONY: first-run-steps
first-run-steps: install env-check docker-up db-wait db-migrate db-dev-setup
	@echo ""
	@echo "  ✓ bootstrap complete."
	@echo "    Next: make up   (the server and the console in one terminal)"
	@echo ""

.PHONY: doctor
doctor:
	@printf "%-16s" "rust:";          command -v rustc      > /dev/null 2>&1 && rustc --version      || echo "MISSING  (install: https://rustup.rs)"
	@printf "%-16s" "cargo:";         command -v cargo      > /dev/null 2>&1 && cargo --version      || echo "MISSING"
	@printf "%-16s" "node:";          command -v node       > /dev/null 2>&1 && node --version       || echo "MISSING  (install: nvm install 24)"
	@printf "%-16s" "npm:";           command -v npm        > /dev/null 2>&1 && npm --version        || echo "MISSING"
	@printf "%-16s" "docker:";        command -v docker     > /dev/null 2>&1 && docker --version     || echo "MISSING  (install: https://docs.docker.com/get-docker/)"
	@printf "%-16s" "docker-compose:"; docker compose version > /dev/null 2>&1 && docker compose version | head -1 || echo "MISSING"
	@printf "%-16s" "docker daemon:"; docker ps             > /dev/null 2>&1 && echo "running"      || echo "NOT RUNNING  (start docker)"
	@printf "%-16s" "gh:";            command -v gh         > /dev/null 2>&1 && gh --version | head -1 || echo "MISSING  (PR/issue work only)"
	@printf "%-16s" ".env:";          test -f .env                       && echo "present"           || echo "MISSING  (cp .env.example .env)"
	@printf "%-16s" "web/.env.local:"; test -f $(WEB_DIR)/.env.local      && echo "present"           || echo "MISSING  (cp $(WEB_DIR)/.env.local.example $(WEB_DIR)/.env.local)"

.PHONY: web-dev
web-dev:
	cd $(WEB_DIR) && npm run dev

.PHONY: web-prod
web-prod:
	cd $(WEB_DIR) && npm run build && PORT=$(WEB_PORT) npm run start

.PHONY: server-dev
server-dev:
	PORT=$(SERVER_PORT) cargo run -p telmoni -- serve

.PHONY: sweep
sweep:
	@test -n "$(SWEEP)" || { echo "usage: make sweep SWEEP=<deletion|audit-verify|retention|agent-reindex|audit-exports>"; exit 2; }
	cargo run -p telmoni -- sweep $(SWEEP)

# ── Quality & Verification ─────────────────────────────────────

.PHONY: check
check:
	cd $(WEB_DIR) && npm run typecheck
	cargo check --workspace --locked
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets --locked -- -D warnings
	@$(MAKE) doc

.PHONY: doc
doc:
	RUSTDOCFLAGS="-D warnings" cargo doc --workspace --locked --no-deps --document-private-items --quiet

.PHONY: ci
ci: check lint test web-gate deny typos

.PHONY: web-gate
web-gate:
	cd $(WEB_DIR) && npm run knip
	cd $(WEB_DIR) && npm run build

.PHONY: lint
lint:
	cd $(WEB_DIR) && npm run lint

.PHONY: test
test: docker-up db-wait
	cd $(WEB_DIR) && npm run test
	cargo test --workspace --locked --no-fail-fast -- --test-threads=$(TEST_THREADS)

.PHONY: e2e
e2e: deps-check docker-up db-wait schema-check
	cargo build -p telmoni
	cd $(WEB_DIR) && npm run test:e2e

.PHONY: contract
contract:
	UPDATE_CONTRACT=1 cargo test -p telmoni-shared --test wire_contract -- --exact wire_contract_is_in_sync
	UPDATE_CONTRACT=1 cargo test -p telmoni-shared --test openapi_contract -- --exact openapi_contract_is_in_sync
	@echo "wrote contract/wire-contract.json + contract/openapi.json"

.PHONY: test-svc
test-svc:
	cargo test -p $(if $(filter telmoni,$(SVC)),telmoni,telmoni-$(SVC)) $(if $(TEST_BIN),--test $(TEST_BIN)) --no-fail-fast -- --test-threads=$(TEST_THREADS) $(TEST_FILTER)

.PHONY: deny
deny:
	cargo deny --all-features check

.PHONY: typos
typos:
	typos

.PHONY: fmt
fmt:
	cargo fmt --all
	cd $(WEB_DIR) && npx prettier --write .

# ── Database ───────────────────────────────────────────────────

.PHONY: migration-sets
migration-sets:
	@echo "$(MIGRATION_SETS)"

.PHONY: roles-check
roles-check:
	@if ! missing=$$(docker compose exec -T postgres psql -U telmoni -d telmoni -tAc \
		"SELECT coalesce(string_agg(r, ' '), '') FROM unnest(ARRAY['auth','notifications','agent','migrator']) AS r WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r)" \
		2>/dev/null); then \
		echo "· roles-check skipped (postgres not reachable)"; \
	elif [ -n "$$missing" ]; then \
		echo "✗ database is missing the roles: $$missing"; \
		echo "  They are created when the postgres volume is first initialised, so"; \
		echo "  this volume predates them. Recreate it (every local row is wiped):"; \
		echo "    make db-reset"; \
		exit 1; \
	else \
		echo "✓ roles present — auth, notifications, agent, migrator."; \
	fi

.PHONY: db-rotate
db-rotate:
	cargo run -p telmoni -- rotate

.PHONY: db-migrate
db-migrate: roles-check
	MIGRATION_SETS="$(MIGRATION_SETS)" cargo run -p telmoni -- migrate

.PHONY: db-dev-setup
db-dev-setup:
	@docker compose exec -T postgres psql -U telmoni -d telmoni \
		-c "ALTER ROLE telmoni SET search_path = auth, notifications, agent, audit, public;"
	@echo "✓ dev role search_path pinned (auth, notifications, agent, audit, public)"

.PHONY: db-shell
db-shell:
	@if command -v psql >/dev/null 2>&1; then \
		psql $(DATABASE_URL); \
	else \
		docker compose exec postgres psql -U telmoni -d telmoni; \
	fi

.PHONY: db-wait
db-wait:
	@echo "▶ Waiting for postgres to finish initialising..."
	@ok=0; n=0; \
	while [ $$n -lt 90 ]; do \
		if docker compose exec -T postgres psql -U telmoni -d telmoni -tAc 'SELECT 1' >/dev/null 2>&1; then \
			ok=$$((ok + 1)); \
			if [ $$ok -ge 3 ]; then echo "✓ postgres ready"; exit 0; fi; \
		else \
			ok=0; \
		fi; \
		n=$$((n + 1)); \
		sleep 1; \
	done; \
	echo "✗ postgres did not become ready within 90s"; exit 1

.PHONY: db-reset
db-reset:
	docker compose down -v
	@$(MAKE) docker-up
	@echo "postgres + redis recreated — all local data wiped"
	$(MAKE) db-wait
	$(MAKE) db-migrate
	$(MAKE) db-dev-setup
	@echo "✓ db-reset complete — postgres + redis fresh, schema migrated, dev role search_path pinned"

# ── Docker ─────────────────────────────────────────────────────

.PHONY: docker-up
docker-up:
	docker compose up -d
	@echo "postgres :5432  redis :6379  mailpit :8025$$(docker compose ps --status running --services 2>/dev/null | grep -qx ollama && echo '  ollama :11434')"

.PHONY: down
down:
	docker compose down

# ── Feature Flags ──────────────────────────────────────────────

.PHONY: flag flags
flag:
	@test -n "$$KEY" -a -n "$$ON" -a -n "$$WHY" -a -n "$$BY" || { \
		echo 'usage: make flag KEY=<flag> ON=true|false WHY="<reason>" BY=<who> [ORG=<org id>]'; exit 2; }
	@case "$$ON" in true|false) ;; *) echo "ON must be true or false"; exit 2;; esac
	@node -e 'const c=require("./contract/wire-contract.json"); const k=process.env.KEY; const org=process.env.ORG; if(!c.enums.Flag.includes(k)){console.error("unknown flag: "+k+"\nknown: "+c.enums.Flag.join(", "));process.exit(2)} if(org && c.flags.global_only.includes(k)){console.error(k+" is global-only: an ORG= row would be read by nothing");process.exit(2)}'
	@org="$$ORG"; \
	if [ -n "$$org" ]; then \
		printf '%s\n' "INSERT INTO auth.organization_flags (organization_id, key, enabled, note, actor) VALUES (:'org', :'key', :'on'::boolean, :'why', :'by');" \
		| sh -c '$(PSQL_AUTH)' sh -v org="$$org" -v key="$$KEY" -v on="$$ON" -v why="$$WHY" -v by="$$BY" \
		&& echo "$$KEY = $$ON for organization $$org (by $$BY)"; \
	else \
		printf '%s\n' "INSERT INTO auth.feature_flags (key, enabled, note, actor) VALUES (:'key', :'on'::boolean, :'why', :'by');" \
		| sh -c '$(PSQL_AUTH)' sh -v key="$$KEY" -v on="$$ON" -v why="$$WHY" -v by="$$BY" \
		&& echo "$$KEY = $$ON globally (by $$BY)"; \
	fi

flags:
	@keys="$$(node -e 'console.log(require("./contract/wire-contract.json").enums.Flag.join(","))')"; \
	 gonly="$$(node -e 'console.log(require("./contract/wire-contract.json").flags.global_only.join(","))')"; \
	 echo "-- every flag, as the resolver answers it globally (switched-off first)"; \
	 printf '%s\n' "WITH catalog AS (SELECT unnest(string_to_array(:'keys', ',')) AS key), g AS (SELECT DISTINCT ON (key) key, enabled, actor, created_at, note FROM auth.feature_flags ORDER BY key, created_at DESC, id DESC) SELECT c.key, COALESCE(g.enabled, true) AS resolved, CASE WHEN g.enabled IS NULL THEN 'default' ELSE 'global row' END AS decided_by, g.actor, g.created_at, g.note FROM catalog c LEFT JOIN g ON g.key = c.key ORDER BY COALESCE(g.enabled, true), c.key;" \
	 | sh -c '$(PSQL_AUTH)' sh -v keys="$$keys"; \
	 echo "-- per organization: every key it resolves differently, or resolves OFF"; \
	 printf '%s\n' "WITH catalog AS (SELECT unnest(string_to_array(:'keys', ',')) AS key), orgs AS (SELECT DISTINCT organization_id FROM auth.organization_flags), g AS (SELECT DISTINCT ON (key) key, enabled FROM auth.feature_flags ORDER BY key, created_at DESC, id DESC), a AS (SELECT DISTINCT ON (organization_id, key) organization_id, key, enabled, actor, created_at, note FROM auth.organization_flags ORDER BY organization_id, key, created_at DESC, id DESC) SELECT ac.organization_id, c.key, COALESCE(a.enabled, g.enabled, true) AS resolved, CASE WHEN a.enabled IS NOT NULL THEN 'organization row' WHEN g.enabled IS NOT NULL THEN 'global row' ELSE 'default' END AS decided_by, a.actor, a.created_at, a.note FROM orgs ac CROSS JOIN catalog c LEFT JOIN a ON a.organization_id = ac.organization_id AND a.key = c.key LEFT JOIN g ON g.key = c.key WHERE a.enabled IS NOT NULL OR COALESCE(g.enabled, true) = false ORDER BY ac.organization_id, COALESCE(a.enabled, g.enabled, true), c.key;" \
	 | sh -c '$(PSQL_AUTH)' sh -v keys="$$keys"; \
	 echo "-- stray rows: written, and read by nobody or by only half the readers"; \
	 printf '%s\n' "SELECT 'organization row on a global-only key' AS why, organization_id, key, enabled, actor, created_at FROM (SELECT DISTINCT ON (organization_id, key) organization_id, key, enabled, actor, created_at FROM auth.organization_flags ORDER BY organization_id, key, created_at DESC, id DESC) s WHERE key = ANY(string_to_array(:'gonly', ',')) UNION ALL SELECT 'key not in the catalog', organization_id, key, enabled, actor, created_at FROM auth.organization_flags WHERE NOT (key = ANY(string_to_array(:'keys', ','))) UNION ALL SELECT 'key not in the catalog', '(global)', key, enabled, actor, created_at FROM auth.feature_flags WHERE NOT (key = ANY(string_to_array(:'keys', ','))) ORDER BY 1, 2, 3;" \
	 | sh -c '$(PSQL_AUTH)' sh -v keys="$$keys" -v gonly="$$gonly"

# ── Webhooks & Operations ──────────────────────────────────────

.PHONY: webhook-receiver
webhook-receiver:
	@TELMONI_WEBHOOK_SECRET="$(or $(SECRET),$(TELMONI_WEBHOOK_SECRET))" \
	 node scripts/webhook-receiver.mjs

.PHONY: webhook-tunnel
webhook-tunnel:
	@bash scripts/webhook-tunnel.sh

.PHONY: log-level
log-level:
	@secret="$$(grep -m1 '^SERVICE_SECRET=' .env | cut -d= -f2- | tr -d ' ')"; \
	base="http://localhost:$(SERVER_PORT)/internal/log-level"; \
	if [ -z "$(LEVEL)" ]; then \
		curl -sS -H "x-service-secret: $$secret" "$$base"; echo; \
	elif [ "$(LEVEL)" = "reset" ]; then \
		curl -sS -X DELETE -H "x-service-secret: $$secret" "$$base"; echo; \
	else \
		curl -sS -X PUT -H "x-service-secret: $$secret" -H 'content-type: application/json' \
			-d "{\"filter\":\"$(LEVEL)\"$(if $(TTL),$(COMMA)\"ttl_secs\":$(TTL))}" "$$base"; echo; \
	fi

.PHONY: clean
clean:
	rm -rf $(WEB_DIR)/node_modules $(WEB_DIR)/.next
	cargo clean

.PHONY: relocated
relocated:
	rm -rf $(WEB_DIR)/.next
	cargo clean
	@echo "✓ stale path caches cleared (web/.next + target/)."
	@echo "  Next: make up   (the first build is from scratch)"
