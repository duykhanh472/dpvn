#!/usr/bin/env bash
# Restore a database dump (and optionally page files) produced by backup.sh.
#
# Usage:
#   DP_RESTORE_CONFIRM=YES DP_RESTORE_DB=dp_restore \
#     ./deploy/restore.sh backups/db-20260101T000000Z.sql.gz [backups/files-....tar.gz]
#
# Safety:
#   * Restoring into the live database (`dp`) requires DP_RESTORE_CONFIRM=YES.
#   * By default the target is `dp_restore`, so a drill cannot overwrite prod.
#   * Existing objects in the target are dropped, so the result is exactly the
#     backup, not a merge.
#
# Environment:
#   DP_RESTORE_DB         target database (default: dp_restore)
#   DP_RESTORE_CONFIRM    must be YES to write to the live `dp` database
#   DP_RESTORE_DATA_DIR   where page files are extracted (default: /tmp/dp-restore)
#   DB_USER/DB_PASSWORD   credentials (default: dp / required)
#   DP_DB_CONTAINER       existing MySQL container name
#   DP_COMPOSE_FILE       compose file used to resolve the db container
#   DB_HOST/DB_PORT       host mode connection target (default: 127.0.0.1:3306)

set -euo pipefail
IFS=$'\n\t'

DB_NAME=${DB_NAME:-dp}
DB_USER=${DB_USER:-dp}
DB_PASSWORD=${DB_PASSWORD:-}
DB_HOST=${DB_HOST:-127.0.0.1}
DB_PORT=${DB_PORT:-3306}
RESTORE_DB=${DP_RESTORE_DB:-dp_restore}
RESTORE_DATA_DIR=${DP_RESTORE_DATA_DIR:-/tmp/dp-restore}

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%SZ)" "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit "${2:-1}"; }

[[ $# -ge 1 ]] || die "usage: $0 <db-backup.sql.gz> [files-backup.tar.gz]"
DB_BACKUP=$1
FILES_BACKUP=${2:-}

[[ -f "$DB_BACKUP" ]] || die "database backup not found: $DB_BACKUP"
[[ -n "$DB_PASSWORD" ]] || die "DB_PASSWORD is required"
[[ "$RESTORE_DB" =~ ^[A-Za-z0-9_]+$ ]] || die "invalid database name: $RESTORE_DB"

if [[ "$RESTORE_DB" == "$DB_NAME" && "${DP_RESTORE_CONFIRM:-}" != "YES" ]]; then
    die "refusing to overwrite live database '$DB_NAME'; set DP_RESTORE_CONFIRM=YES"
fi

if [[ -f "$DB_BACKUP.sha256" ]]; then
    log "verifying checksum"
    (cd "$(dirname "$DB_BACKUP")" && sha256sum -c "$(basename "$DB_BACKUP").sha256")
fi

resolve_db_container() {
    if [[ -n "${DP_DB_CONTAINER:-}" ]]; then
        printf '%s' "$DP_DB_CONTAINER"
    elif [[ -n "${DP_COMPOSE_FILE:-}" ]]; then
        docker compose -f "$DP_COMPOSE_FILE" ps -q db
    fi
}

DB_CONTAINER=$(resolve_db_container)

db_client() {
    if [[ -n "$DB_CONTAINER" ]]; then
        docker exec -i -e MYSQL_PWD="$DB_PASSWORD" "$DB_CONTAINER" \
            mysql -h 127.0.0.1 -u "$DB_USER" "$@"
    else
        MYSQL_PWD="$DB_PASSWORD" mysql -h "$DB_HOST" -P "$DB_PORT" -u "$DB_USER" "$@"
    fi
}

log "creating target database '$RESTORE_DB'"
db_client -e "DROP DATABASE IF EXISTS \`$RESTORE_DB\`;
CREATE DATABASE \`$RESTORE_DB\` CHARACTER SET utf8mb4 COLLATE utf8mb4_vietnamese_ci;"

log "restoring $DB_BACKUP into $RESTORE_DB"
gunzip -c "$DB_BACKUP" | db_client "$RESTORE_DB"

log "verifying restored row counts"
db_client -N -B -e "SELECT CONCAT('users=', COUNT(*)) FROM \`$RESTORE_DB\`.users;
SELECT CONCAT('projects=', COUNT(*)) FROM \`$RESTORE_DB\`.projects;
SELECT CONCAT('pages=', COUNT(*)) FROM \`$RESTORE_DB\`.pages;
SELECT CONCAT('page_rounds=', COUNT(*)) FROM \`$RESTORE_DB\`.page_rounds;"

if [[ -n "$FILES_BACKUP" ]]; then
    [[ -f "$FILES_BACKUP" ]] || die "file backup not found: $FILES_BACKUP"
    if [[ -f "$FILES_BACKUP.sha256" ]]; then
        log "verifying file checksum"
        (cd "$(dirname "$FILES_BACKUP")" && sha256sum -c "$(basename "$FILES_BACKUP").sha256")
    fi
    log "extracting page files into $RESTORE_DATA_DIR"
    mkdir -p "$RESTORE_DATA_DIR"
    tar -C "$RESTORE_DATA_DIR" -xzf "$FILES_BACKUP"
    log "page files restored: $(du -sh "$RESTORE_DATA_DIR" | cut -f1)"
else
    log "no page-file archive supplied; skipping file restore"
fi

log "restore complete"
log "next: point the app at '$RESTORE_DB', run 'dp verify', and smoke-test login/proofing"
