#!/usr/bin/env bash
# Nightly logical + file backup for Distributed Proofreaders.
#
# Usage:
#   DB_PASSWORD=... ./deploy/backup.sh
#
# The script supports two database access modes:
#   * host mode:  set DP_DB_HOST/DP_DB_PORT and have mysql-client installed
#   * docker mode: set DP_DB_CONTAINER or DP_COMPOSE_FILE (recommended)
#
# Files are read from DP_DATA_DIR, or from the named volume DP_DATA_VOLUME
# using a throwaway alpine container (recommended in production).
#
# Environment:
#   BACKUP_DIR        output directory (default: ./backups)
#   DB_NAME           database name (default: dp)
#   DB_USER           database user (default: dp)
#   DB_PASSWORD       database password (required)
#   DB_HOST/DB_PORT   host mode connection target (default: 127.0.0.1:3306)
#   DP_DB_CONTAINER   existing MySQL container name
#   DP_COMPOSE_FILE   compose file used to resolve the db container
#   DP_DATA_DIR       host path containing images/ and display/
#   DP_DATA_VOLUME    docker volume name containing images/ and display/
#   RETENTION_DAYS    delete local backups older than N days (default: 14)
#
# Exit codes: 0 success, 1 usage/configuration error, 2 backup failure.

set -euo pipefail
IFS=$'\n\t'

BACKUP_DIR=${BACKUP_DIR:-./backups}
DB_NAME=${DB_NAME:-dp}
DB_USER=${DB_USER:-dp}
DB_PASSWORD=${DB_PASSWORD:-}
DB_HOST=${DB_HOST:-127.0.0.1}
DB_PORT=${DB_PORT:-3306}
RETENTION_DAYS=${RETENTION_DAYS:-14}
STAMP=$(date -u +%Y%m%dT%H%M%SZ)

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%SZ)" "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit "${2:-1}"; }

[[ -n "$DB_PASSWORD" ]] || die "DB_PASSWORD is required"
mkdir -p "$BACKUP_DIR"
chmod 700 "$BACKUP_DIR"

resolve_db_container() {
    if [[ -n "${DP_DB_CONTAINER:-}" ]]; then
        printf '%s' "$DP_DB_CONTAINER"
    elif [[ -n "${DP_COMPOSE_FILE:-}" ]]; then
        docker compose -f "$DP_COMPOSE_FILE" ps -q db
    fi
}

DB_CONTAINER=$(resolve_db_container)

db_dump() {
    if [[ -n "$DB_CONTAINER" ]]; then
        docker exec -i -e MYSQL_PWD="$DB_PASSWORD" "$DB_CONTAINER" \
            mysqldump --single-transaction --quick --default-character-set=utf8mb4 \
            -h 127.0.0.1 -u "$DB_USER" "$DB_NAME"
    else
        MYSQL_PWD="$DB_PASSWORD" mysqldump --single-transaction --quick \
            --default-character-set=utf8mb4 -h "$DB_HOST" -P "$DB_PORT" \
            -u "$DB_USER" "$DB_NAME"
    fi
}

DB_BACKUP="$BACKUP_DIR/db-$STAMP.sql.gz"
log "dumping database '$DB_NAME' to $DB_BACKUP"
db_dump | gzip -9 > "$DB_BACKUP"
gzip -t "$DB_BACKUP"
sha256sum "$DB_BACKUP" > "$DB_BACKUP.sha256"
log "database backup complete ($(du -h "$DB_BACKUP" | cut -f1))"

FILE_BACKUP=""
if [[ -n "${DP_DATA_VOLUME:-}" ]]; then
    FILE_BACKUP="$BACKUP_DIR/files-$STAMP.tar.gz"
    log "archiving docker volume '$DP_DATA_VOLUME' to $FILE_BACKUP"
    docker run --rm \
        -v "$DP_DATA_VOLUME":/data:ro \
        -v "$(cd "$BACKUP_DIR" && pwd)":/backup \
        alpine:3.20 \
        tar -C /data -czf "/backup/files-$STAMP.tar.gz" images display 2>/dev/null || {
            log "warning: volume archive skipped (images/ or display/ missing)"
            rm -f "$FILE_BACKUP"
            FILE_BACKUP=""
        }
elif [[ -n "${DP_DATA_DIR:-}" && -d "$DP_DATA_DIR" ]]; then
    FILE_BACKUP="$BACKUP_DIR/files-$STAMP.tar.gz"
    log "archiving '$DP_DATA_DIR' to $FILE_BACKUP"
    tar -C "$DP_DATA_DIR" -czf "$FILE_BACKUP" images display 2>/dev/null || {
        log "warning: file archive skipped (images/ or display/ missing)"
        rm -f "$FILE_BACKUP"
        FILE_BACKUP=""
    }
else
    log "warning: neither DP_DATA_VOLUME nor DP_DATA_DIR is set; skipping page files"
fi

if [[ -n "$FILE_BACKUP" ]]; then
    sha256sum "$FILE_BACKUP" > "$FILE_BACKUP.sha256"
    log "file backup complete ($(du -h "$FILE_BACKUP" | cut -f1))"
fi

# Optional off-host copy.
if [[ -n "${RESTIC_REPOSITORY:-}" ]] && command -v restic >/dev/null 2>&1; then
    log "copying backups to restic repository"
    restic backup --tag dproofreaders "$DB_BACKUP"
    [[ -n "$FILE_BACKUP" ]] && restic backup --tag dproofreaders "$FILE_BACKUP"
fi

log "pruning local backups older than $RETENTION_DAYS days"
find "$BACKUP_DIR" -maxdepth 1 -type f \( -name 'db-*.sql.gz' -o -name 'files-*.tar.gz' \) \
    -mtime "+$RETENTION_DAYS" -print -delete || true
find "$BACKUP_DIR" -maxdepth 1 -type f -name '*.sha256' \
    -mtime "+$RETENTION_DAYS" -print -delete || true

log "backup finished: $DB_BACKUP${FILE_BACKUP:+ and $FILE_BACKUP}"
