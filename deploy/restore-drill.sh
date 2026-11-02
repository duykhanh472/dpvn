#!/usr/bin/env bash
# End-to-end restore drill.
#
# Starts a throwaway MySQL 8.4 container, applies the real migrations, inserts a
# small fixture, runs deploy/backup.sh, restores into a second database with
# deploy/restore.sh, and compares per-table checksums.
#
# Requires: docker, gzip, sha256sum.
#
# Usage:
#   ./deploy/restore-drill.sh            # prints a report and exits 0 on match
#   DRILL_LOG=docs/operations/restore-drill-log.md ./deploy/restore-drill.sh
#
# This is the script referenced by the Phase 11 acceptance criterion
# "restore from backup into a clean environment reproduces the site".

set -euo pipefail
IFS=$'\n\t'

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
NAME="dp-restore-drill-$$"
TMP=$(mktemp -d "${TMPDIR:-/tmp}/dp-drill.XXXXXX")
STARTED=$(date -u +%Y-%m-%dT%H:%M:%SZ)

log() { printf '[drill] %s\n' "$*"; }
die() { printf '[drill] error: %s\n' "$*" >&2; exit 1; }

cleanup() {
    docker rm -f "$NAME" >/dev/null 2>&1 || true
    rm -rf "$TMP"
}
trap cleanup EXIT

command -v docker >/dev/null 2>&1 || die "docker is required"

log "starting throwaway MySQL container $NAME"
docker run -d --rm --name "$NAME" \
    -e MYSQL_ROOT_PASSWORD=drill \
    -e MYSQL_DATABASE=dp_src \
    mysql:8.4 \
    --character-set-server=utf8mb4 \
    --collation-server=utf8mb4_vietnamese_ci >/dev/null

log "waiting for MySQL to accept connections"
ready=0
for _ in $(seq 1 90); do
    if docker exec -e MYSQL_PWD=drill "$NAME" mysqladmin ping -h 127.0.0.1 -uroot --silent >/dev/null 2>&1; then
        ready=1
        break
    fi
    sleep 1
done
[[ "$ready" == "1" ]] || die "MySQL did not become ready"

mysql_exec() {
    docker exec -i -e MYSQL_PWD=drill "$NAME" mysql -h 127.0.0.1 -uroot "$@"
}

log "applying migrations to dp_src"
for migration in "$ROOT_DIR"/crates/dp-db/migrations/*.sql; do
    log "  $(basename "$migration")"
    mysql_exec dp_src < "$migration"
done

log "inserting fixture rows"
mysql_exec dp_src <<'SQL'
INSERT INTO users (username, email, password_hash, status, created_at)
VALUES ('drill_proofer', 'drill@example.org', 'not-a-real-hash', 'active', NOW(6));

INSERT INTO projects
  (name, author, phase, current_round, manager_id, comments, created_at, updated_at)
VALUES
  ('Sách kiểm tra khôi phục', 'Tác giả', 'in_round', 'P1', 1, 'restore drill', NOW(6), NOW(6));

INSERT INTO pages (project_id, page_number, image_path, image_width, image_height, ocr_text)
VALUES (1, 1, 'images/1/1.png', 1500, 2200, 'Văn bản OCR mẫu');

INSERT INTO page_rounds (page_id, round, state, text)
VALUES (1, 'P1', 'available', 'Văn bản đã soát');

INSERT INTO project_events (project_id, from_phase, to_phase, actor_id, note, created_at)
VALUES (1, 'draft', 'in_round', 1, 'drill', NOW(6));

INSERT INTO settings (k, v, updated_at) VALUES ('registration_open', '1', NOW(6));
SQL

checksum_table() {
    local db=$1 table=$2
    mysql_exec -N -B -e "CHECKSUM TABLE \`$db\`.\`$table\`" | awk '{print $2}'
}

TABLES=(users projects pages page_rounds project_events settings)
declare -A SRC_CHECKSUMS
for table in "${TABLES[@]}"; do
    SRC_CHECKSUMS[$table]=$(checksum_table dp_src "$table")
done

log "running backup.sh"
DP_DB_CONTAINER="$NAME" \
DB_USER=root \
DB_PASSWORD=drill \
DB_NAME=dp_src \
BACKUP_DIR="$TMP" \
    "$ROOT_DIR/deploy/backup.sh" >/dev/null

DB_BACKUP=$(find "$TMP" -maxdepth 1 -name 'db-*.sql.gz' -print -quit)
[[ -n "$DB_BACKUP" ]] || die "backup.sh produced no database dump"

log "running restore.sh into dp_restore"
DP_DB_CONTAINER="$NAME" \
DB_USER=root \
DB_PASSWORD=drill \
DB_NAME=dp_src \
DP_RESTORE_DB=dp_restore \
DP_RESTORE_CONFIRM=YES \
    "$ROOT_DIR/deploy/restore.sh" "$DB_BACKUP" >/dev/null

log "comparing checksums"
mismatch=0
report=""
for table in "${TABLES[@]}"; do
    restored=$(checksum_table dp_restore "$table")
    if [[ "$restored" == "${SRC_CHECKSUMS[$table]}" ]]; then
        status="OK"
    else
        status="MISMATCH"
        mismatch=1
    fi
    # shellcheck disable=SC2016 # backticks are Markdown, not command substitution
    line=$(printf '| `%s` | `%s` | `%s` | %s |' "$table" "${SRC_CHECKSUMS[$table]}" "$restored" "$status")
    report+="$line"$'\n'
done

printf '\nRestore drill report\n'
printf 'Started: %s\n' "$STARTED"
printf 'Finished: %s\n\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '| Table | Source checksum | Restored checksum | Result |\n'
printf '|---|---|---|---|\n'
printf '%s' "$report"

if [[ -n "${DRILL_LOG:-}" ]]; then
    mkdir -p "$(dirname "$DRILL_LOG")"
    {
        printf '\n## Drill run %s\n\n' "$STARTED"
        printf '| Table | Source checksum | Restored checksum | Result |\n'
        printf '|---|---|---|---|\n'
        printf '%s' "$report"
    } >> "$DRILL_LOG"
    log "appended report to $DRILL_LOG"
fi

if [[ "$mismatch" -ne 0 ]]; then
    die "restore drill FAILED: at least one table checksum differs"
fi

log "restore drill PASSED: every table checksum matches"
