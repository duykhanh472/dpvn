# Backup and restore

Backups cover two things: the MySQL database (all proofing state) and the
immutable page images under `/var/lib/dp`. `display/` derivatives are
regenerable and may be excluded if space is tight.

| Target | RPO | RTO | Method |
|---|---|---|---|
| Database | 24 h (nightly) | 1 h | `mysqldump --single-transaction` |
| Page images | 24 h | 2 h | `tar` of the `dp-data` volume |
| Off-host copy | 24 h | — | restic repository (optional) |

The Phase 11 acceptance criterion is a **performed** restore drill. The script
`deploy/restore-drill.sh` automates it and appends a signed report to
`docs/operations/restore-drill-log.md`.

## 1. Nightly backup

```sh
# From the repository root, with the production compose file.
DB_PASSWORD="$(grep '^DB_PASSWORD=' .env | cut -d= -f2-)" \
DP_COMPOSE_FILE=docker-compose.prod.yml \
DP_DATA_VOLUME="$(docker volume ls -q | grep dp-data | head -1)" \
BACKUP_DIR=/srv/backups \
  ./deploy/backup.sh
```

Cron (03:15 UTC daily):

```cron
15 3 * * * cd /opt/dproofreaders && set -a && . ./.env && set +a && \
  DP_COMPOSE_FILE=docker-compose.prod.yml DP_DATA_VOLUME=dproofreaders-rust_dp-data \
  BACKUP_DIR=/srv/backups ./deploy/backup.sh >> /var/log/dp-backup.log 2>&1
```

What the script writes:

```
/srv/backups/db-20260101T031500Z.sql.gz
/srv/backups/db-20260101T031500Z.sql.gz.sha256
/srv/backups/files-20260101T031500Z.tar.gz
/srv/backups/files-20260101T031500Z.tar.gz.sha256
```

Local backups older than `RETENTION_DAYS` (default 14) are pruned. Set
`RESTIC_REPOSITORY` to also push them off-host with restic.

### Verify a backup without restoring

```sh
gzip -t backups/db-*.sql.gz
(cd backups && sha256sum -c db-*.sql.gz.sha256)
```

## 2. Restore drill (required before launch)

The drill runs entirely in a throwaway MySQL container. It applies the real
migrations, inserts a fixture, backs it up, restores it into a second database,
and compares per-table checksums.

```sh
./deploy/restore-drill.sh
# Optional: append the report to the audit log.
DRILL_LOG=docs/operations/restore-drill-log.md ./deploy/restore-drill.sh
```

Exit code 0 means every table checksum matched. Any mismatch exits non-zero.
Record the run date, operator, and report in `restore-drill-log.md`.

## 3. Restore into a scratch database (safe)

```sh
DP_DB_CONTAINER="$(docker compose -f docker-compose.prod.yml ps -q db)" \
DB_USER=root DB_PASSWORD="$DB_ROOT_PASSWORD" DB_NAME=dp \
DP_RESTORE_DB=dp_restore \
  ./deploy/restore.sh backups/db-20260101T031500Z.sql.gz
```

Point a staging app at `dp_restore` and run `dp verify` plus a login/proofing
smoke test. This is how you validate a backup without touching production.

## 4. Restore into production

Only after the scratch restore has been verified:

```sh
# 1. Put the site in maintenance mode / stop the app.
docker compose -f docker-compose.prod.yml stop app web

# 2. Restore the database and files.
DP_DB_CONTAINER="$(docker compose -f docker-compose.prod.yml ps -q db)" \
DB_USER=root DB_PASSWORD="$DB_ROOT_PASSWORD" DB_NAME=dp \
DP_RESTORE_DB=dp DP_RESTORE_CONFIRM=YES \
DP_RESTORE_DATA_DIR=/var/lib/dp-restore \
  ./deploy/restore.sh backups/db-20260101T031500Z.sql.gz \
                    backups/files-20260101T031500Z.tar.gz

# 3. Swap the restored files into place (keep the old directory for rollback).
docker run --rm -v dproofreaders-rust_dp-data:/data -v /var/lib/dp-restore:/restore \
  alpine:3.20 sh -c 'mv /data/images /data/images.old && mv /restore/images /data/images'

# 4. Start and verify.
docker compose -f docker-compose.prod.yml up -d
curl -fsS https://proof.example.org/readyz
```

Then verify row counts and a real proofing save:

```sql
SELECT COUNT(*) FROM users;
SELECT COUNT(*) FROM projects;
SELECT COUNT(*) FROM pages;
SELECT COUNT(*) FROM page_rounds;
```

## 5. What is not backed up

- `/etc/dp/config.toml`, `.env`, and TLS certificates — store them in your
  secrets manager; they are intentionally excluded from the image and backups.
- Redis or other caches — none are used.
- `display/` derivatives — regenerable from `images/`.
- Logs — shipped to stdout and collected by the platform, not backed up.

## 6. Failure modes

| Symptom | Cause | Action |
|---|---|---|
| `mysqldump` exits 2 | bad credentials or DB down | check `DB_PASSWORD`, `docker compose ps db` |
| dump is 20 bytes | `mysqldump` wrote an error to stdout | re-run without `>/dev/null` and inspect stderr |
| restore checksum mismatch | truncated/corrupt backup | try the previous night; investigate disk |
| `dp verify` fails after restore | migration version newer than binary | deploy the matching binary or run `dp migrate` |
| images 404 after restore | volume ownership changed | `chown -R 65532:65532 /var/lib/dp` |
