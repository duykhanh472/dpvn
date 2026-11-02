# Operations runbook

On-call guide for the five most likely incidents, plus the generic “site is
down” path. Every command assumes the production compose file and a shell in
the repository root with `.env` loaded:

```sh
cd /opt/dproofreaders
set -a; . ./.env; set +a
COMPOSE="docker compose -f docker-compose.prod.yml"
```

Useful aliases:

```sh
logs() { $COMPOSE logs --since "${1:-15m}" app; }
db()   { $COMPOSE exec -T db mysql -uroot -p"$DB_ROOT_PASSWORD" dp -e "$1"; }
```

---

## 1. Stuck page claims

**Symptoms**

- Proofers report “Bạn không phải người đang giữ trang này” or cannot claim any
  page even though the pool shows available pages.
- `page_rounds.state = 'in_progress'` rows older than the claim timeout.
- The project appears stuck at a round boundary.

**Diagnose**

```sql
-- Claims older than the timeout (default 72 h).
SELECT pr.page_id, pr.round, pr.proofer_id, pr.claimed_at,
       TIMESTAMPDIFF(HOUR, pr.claimed_at, NOW()) AS hours_held
FROM page_rounds pr
WHERE pr.state = 'in_progress'
  AND pr.claimed_at < NOW() - INTERVAL 72 HOUR
ORDER BY pr.claimed_at
LIMIT 50;

-- Is the automatic job running? Look for this line in the last 30 minutes.
-- "released stale page claims"
```

```sh
$COMPOSE logs --since 30m app | grep 'released stale page claims'
```

**Mitigate**

1. Run the maintenance job immediately:

   ```sh
   $COMPOSE run --rm app prune
   ```

2. If a specific proofer is blocked and the job is healthy, release one claim
   after confirming with the proofer:

   ```sql
   UPDATE page_rounds
   SET state = 'available', proofer_id = NULL, claimed_at = NULL
   WHERE page_id = <id> AND round = '<round>' AND state = 'in_progress';
   ```

3. If a whole round is stuck because every page is `done` but the project did
   not advance, use the admin “force transition” action (it is audited) rather
   than editing rows by hand.

**Prevent**

- Keep `dp prune` on a 15-minute timer; alert if the “released stale claims”
  line is absent for 30 minutes.
- Set `claim_timeout_hours` conservatively (72 h is the default) and tell
  proofers about the “Nhả trang” button.

---

## 2. Failed import

**Symptoms**

- The import page shows a validation error, or the CLI exits non-zero.
- The project exists but has 0 pages, or half the pages are missing.
- Disk usage jumped and `tmp/` is full.

**Diagnose**

```sh
# CLI dry run (safe, no writes) — prints page count, warnings, and errors.
$COMPOSE run --rm -v /path/to/book.zip:/tmp/book.zip app \
  import-project /tmp/book.zip --name "Title" --author "Author"

# Check the last import attempt.
$COMPOSE logs --since 1h app | grep -i 'import\|ingest\|zip'
```

```sql
SELECT id, name, phase, created_at FROM projects ORDER BY id DESC LIMIT 5;
SELECT project_id, COUNT(*) FROM pages GROUP BY project_id ORDER BY project_id DESC;
```

**Mitigate**

1. Fix the package (missing image, mismatched OCR, wrong naming) and re-run the
   dry run. Do not retry the web form blindly; the dry run explains the error.
2. If a partial project was created, re-import with `--replace` from the CLI
   (it is idempotent for the same project) or delete the empty draft through
   the admin project list.
3. If `tmp/` filled the disk, remove stale extraction directories (see §3) and
   re-run.

**Prevent**

- Validate packages with `dp import-project` in a staging environment first.
- Keep `client_max_body_size` and `limits.max_upload_mb` in sync (512 MB).
- Alert on disk > 80% and on any 5xx from `/projects/{id}/import`.

---

## 3. Disk full

**Symptoms**

- 5xx on import, save, or image requests; `No space left on device` in logs.
- `df -h` shows the data volume or `/` above 95%.

**Diagnose**

```sh
df -h
docker system df
# Largest directories inside the data volume.
docker run --rm -v dproofreaders-rust_dp-data:/data alpine:3.20 \
  du -h -d 2 /data | sort -h | tail -20
```

**Mitigate (in order)**

1. **Free regenerable space:** remove old `display/` derivatives and `tmp/`
   entries. They are regenerated on demand / re-ingest.

   ```sh
   docker run --rm -v dproofreaders-rust_dp-data:/data alpine:3.20 \
     sh -c 'rm -rf /data/tmp/*; find /data/display -type f -mtime +30 -delete'
   ```

2. **Prune Docker:** `docker system prune -f` (never `--volumes` in
   production).
3. **Archive posted projects:** move `images/<project_id>` for `posted` or
   `archived` projects to cold storage, then remove them. The DB keeps the
   metadata; display images remain for browsing.
4. **Expand the volume** and resize the filesystem, then restart the app.
5. **Last resort:** move `dp-data` to a larger disk and update the compose
   volume mapping. Take a backup first.

**Prevent**

- Alert at 80% and 90%.
- Keep at least 4× steady-state free space (§15.4).
- Run `dp prune` every 15 minutes; it removes orphan files and stale tmp data.

---

## 4. Database slow

**Symptoms**

- p95 save latency > 500 ms, requests time out, pool saturation.
- `SHOW PROCESSLIST` shows many `Sending data` / `Waiting for lock` rows.
- `Innodb_deadlocks` increasing.

**Diagnose**

```sql
SHOW GLOBAL STATUS LIKE 'Threads_connected';
SHOW GLOBAL STATUS LIKE 'Innodb_deadlocks';
SHOW GLOBAL STATUS LIKE 'Innodb_row_lock_waits';
SHOW PROCESSLIST;

-- Long-running transactions.
SELECT trx_id, trx_state, trx_started, trx_mysql_thread_id, trx_query
FROM information_schema.innodb_trx
ORDER BY trx_started
LIMIT 10;
```

```sh
$COMPOSE logs --since 15m app | grep -E 'slow|timeout|deadlock|pool'
```

**Mitigate**

1. If the pool is saturated by slow queries, lower `database.max_connections`
   in `config.toml` to protect MySQL and restart the app; then investigate.
2. Kill a single long transaction after identifying it:
   `KILL <thread_id>;` (never kill the replication/SQL thread).
3. If the buffer pool is too small, increase `--innodb-buffer-pool-size` in
   `docker-compose.prod.yml` and restart MySQL in a maintenance window.
4. For a deadlock storm, capture `SHOW ENGINE INNODB STATUS\G`, then roll back
   the offending deploy. The save path uses a fixed lock order (§6.4); a new
   query that violates it is a code bug.
5. Verify the hot indexes exist (`page_rounds.pr_pool`, `pages.pages_order`,
   `project_events.pe_project`). Missing indexes after a migration are a
   release blocker.

**Prevent**

- Alert on pool saturation, p95 save latency, and deadlock count.
- Load-test every release (`docs/operations/load-test.md`).
- Never add a query that scans `pages.ocr_text` or `page_versions.text`.

---

## 5. High 5xx rate / failed deploy

**Symptoms**

- `/readyz` failing, nginx returning 502/504, or the 5xx rate above 1%.
- A deploy just completed and errors started.

**Diagnose**

```sh
curl -i https://proof.example.org/healthz
curl -i https://proof.example.org/readyz
$COMPOSE ps
$COMPOSE logs --since 10m app | grep -E 'ERROR|error|panic'
$COMPOSE logs --since 10m web | tail -50
```

**Mitigate**

1. **Roll back the binary first.** If the migration was additive, the previous
   image works without a database restore:

   ```sh
   $COMPOSE stop app
   $COMPOSE up -d --no-deps app   # with the previous image tag
   ```

2. If the schema is incompatible, restore the pre-deploy snapshot
   (`docs/operations/backup-restore.md` §4) and then roll back.
3. If nginx is the problem, reload it: `$COMPOSE exec web nginx -t && $COMPOSE exec web nginx -s reload`.
4. If MySQL is down, start it and wait for the healthcheck before starting the
   app: `$COMPOSE up -d db && $COMPOSE up -d app`.
5. Confirm recovery with `/readyz` and a real save, then watch for 15 minutes.

**Prevent**

- Always take a snapshot before `dp migrate`.
- Use the zero-downtime sequence in `docs/operations/deploy.md` §3.
- Keep the previous image tag available for at least one release.

---

## 6. Generic “site is down”

1. `curl -I https://proof.example.org/healthz` — if this fails, check DNS,
   certificate, and `$COMPOSE ps web`.
2. `$COMPOSE ps` — any container restarting or unhealthy?
3. `$COMPOSE logs --since 5m db` — MySQL crash, disk full, or auth failure.
4. `df -h` — disk full is the most common cause of a sudden total outage.
5. `docker events --since 10m` — OOM kills and restart loops.
6. If all else fails, follow the restore procedure on a clean host. The
   restore drill in `docs/operations/restore-drill-log.md` is the proof that
   this works.

## 7. Escalation

| Severity | Definition | Response |
|---|---|---|
| SEV1 | Site down or data loss | Page on-call immediately; restore first, investigate second |
| SEV2 | Proofing or saves failing for many users | Respond within 30 min; roll back if a deploy is implicated |
| SEV3 | Single project/import broken | Respond within 4 h; use admin tools |
| SEV4 | Cosmetic or single-user issue | Next business day |

Record every SEV1/SEV2 in the incident log with: start/end time, impact,
detection source, root cause, mitigation, and follow-up actions.
