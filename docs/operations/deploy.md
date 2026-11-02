# Deployment and migration runbook

This is the production sequence for the distroless image in `Dockerfile` and
the stack in `docker-compose.prod.yml`. It covers first deploy, routine
releases, migrations, rollback, and TLS renewal.

## 1. Prerequisites

- Docker 24+ and the Compose plugin.
- A `.env` file created from `.env.example` with strong secrets.
- TLS certificates in `./certs/fullchain.pem` and `./certs/privkey.pem`.
- `config.toml` with `[server].public_url` set to the real HTTPS origin.
- A `dp-data` volume owned by UID/GID `65532` (the distroless `nonroot` user):

  ```sh
  docker volume create dp-data
  docker run --rm -v dp-data:/data alpine:3.20 chown -R 65532:65532 /data
  ```

The app image runs read-only as UID 65532 with all capabilities dropped. Only
`/var/lib/dp` (the data volume) and `/tmp` (tmpfs) are writable.

## 2. First deploy

```sh
# 1. Build both images (app + nginx with baked static assets).
docker compose -f docker-compose.prod.yml build

# 2. Apply migrations once, before the server starts.
docker compose -f docker-compose.prod.yml run --rm app migrate

# 3. Start the stack.
docker compose -f docker-compose.prod.yml up -d

# 4. Wait for readiness.
until curl -fsS https://proof.example.org/readyz; do sleep 2; done

# 5. Bootstrap the first administrator (prompts or reads DP_ADMIN_PASSWORD).
docker compose -f docker-compose.prod.yml run --rm -e DP_ADMIN_PASSWORD='...' \
  app create-admin admin

# 6. Smoke test: log in, open a project, proof one page, save it.
```

`dp migrate` and `dp create-admin` are one-off containers. They use the same
image and configuration as the server, so there is no separate toolchain to
install on the host.

## 3. Routine release (zero-downtime)

Migrations are forward-only and must be backward compatible with the previous
binary for one release (add columns with defaults; drop old columns one release
later). That makes the following sequence safe.

```sh
# 0. Take a snapshot first. Never migrate without a recent backup.
DB_PASSWORD=... DP_COMPOSE_FILE=docker-compose.prod.yml ./deploy/backup.sh

# 1. Build the new images without touching the running containers.
docker compose -f docker-compose.prod.yml build

# 2. Run migrations while the old binary is still serving. They are additive.
docker compose -f docker-compose.prod.yml run --rm app migrate

# 3. Start a second app container. nginx resolves `app` through Docker DNS
#    (resolver 127.0.0.11) and will send traffic to both replicas.
docker compose -f docker-compose.prod.yml up -d --no-deps --scale app=2 app

# 4. Wait until both replicas are healthy, then verify the new code path.
docker compose -f docker-compose.prod.yml ps
curl -fsS https://proof.example.org/readyz
curl -fsS https://proof.example.org/healthz

# 5. Remove the old replica. The remaining container runs the new image.
docker compose -f docker-compose.prod.yml up -d --no-deps --scale app=1 app

# 6. Watch 5xx rate and p95 save latency for 15 minutes.
```

Notes:

- Compose is not an orchestrator. If you need a guaranteed no-drop rollout,
  run the same image behind a real load balancer or Kubernetes; the migration
  and health-check steps are identical.
- `docker compose up --scale` may briefly recreate a container. Run step 3 at
  a quiet moment if your traffic is bursty.
- The app handles SIGTERM gracefully: in-flight saves finish before exit
  (`stop_grace_period: 45s`).

## 4. Migration runbook

```
1. Take a DB snapshot (deploy/backup.sh).
2. Deploy the new binary WITHOUT starting it (build only).
3. Run `dp migrate`.
4. Start the new binary.
5. `curl -f /readyz` until it returns 200.
6. Watch the error rate for 15 minutes.
7. If it fails: stop the new container, restore the snapshot, redeploy the
   previous image.
```

Rules:

- Additive changes only within a release.
- Destructive changes (drop column/table, narrow a type) land one release
  after the code stops using the old shape.
- Every migration must run on an empty database and on a copy of production.
- `dp migrate` is idempotent; the `_sqlx_migrations` table records applied
  versions.

## 5. Rollback

```sh
# Stop the bad app replica(s).
docker compose -f docker-compose.prod.yml stop app

# Restore the database snapshot taken in step 0 (see backup-restore.md).
DP_RESTORE_CONFIRM=YES DP_RESTORE_DB=dp DB_PASSWORD=... \
  ./deploy/restore.sh backups/db-<timestamp>.sql.gz

# Deploy the previous image tag (tag images in your registry per release).
docker compose -f docker-compose.prod.yml up -d --no-deps app
```

If the migration was additive, the previous binary usually works without a
database restore; prefer rolling back the binary first, then decide whether a
restore is necessary.

## 6. TLS renewal

Certificates are mounted read-only at `/etc/nginx/certs`. Renew them with your
ACME client and reload nginx:

```sh
docker compose -f docker-compose.prod.yml exec web nginx -s reload
```

The HTTP server keeps `/.well-known/acme-challenge/` available for webroot
validation. Verify the new certificate with:

```sh
openssl s_client -connect proof.example.org:443 -servername proof.example.org </dev/null \
  | openssl x509 -noout -dates -subject
```

## 7. Post-deploy verification

| Check | Command | Expected |
|---|---|---|
| Liveness | `curl -fsS /healthz` | `200 OK` |
| Readiness | `curl -fsS /readyz` | `200 READY` |
| Static asset cache | `curl -I /static/css/<hashed>.css` | `Cache-Control: public, immutable` |
| Protected image | `curl -I /protected/images/1/1.png` | `404` (internal only) |
| Security headers | `curl -I /` | CSP with `script-src 'self'` |
| Non-root app | `docker inspect -f '{{.Config.User}}' dproofreaders:latest` | `65532:65532` |
| JSON logs | `docker compose logs app --tail 5` | one JSON object per line |

See `docs/operations/launch-checklist.md` for the full sign-off list.
