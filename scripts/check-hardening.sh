#!/usr/bin/env bash
# Static verification of the Phase 11 hardening checklist.
#
# Run in CI and before a release:
#   ./scripts/check-hardening.sh
#
# It does not replace the dynamic checks (load test, restore drill, a11y
# review); it catches accidental regressions in configuration and templates.

set -uo pipefail
IFS=$'\n\t'

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1

failures=0
pass() { printf '  ok   %s\n' "$*"; }
fail() { printf '  FAIL %s\n' "$*"; failures=$((failures + 1)); }

contains() {
    local file=$1 pattern=$2 label=$3
    if grep -Eq "$pattern" "$file" 2>/dev/null; then
        pass "$label"
    else
        fail "$label ($file)"
    fi
}

missing() {
    local file=$1 pattern=$2 label=$3
    if grep -Eq "$pattern" "$file" 2>/dev/null; then
        fail "$label ($file)"
    else
        pass "$label"
    fi
}

exists() {
    if [[ -e "$1" ]]; then pass "$2"; else fail "$2 ($1 missing)"; fi
}

printf 'Container image\n'
exists Dockerfile "Dockerfile exists"
contains Dockerfile 'gcr\.io/distroless/cc-debian12:nonroot' "runtime is distroless nonroot"
contains Dockerfile 'USER 65532:65532' "app runs as a non-root UID"
contains Dockerfile 'HEALTHCHECK' "image declares a healthcheck"
contains Dockerfile 'dp", "healthcheck' "healthcheck uses the binary (no curl)"
contains Dockerfile 'cargo build --release --locked' "release build is lockfile-pinned"
missing Dockerfile 'apt-get install.*curl' "runtime does not depend on curl"

printf '\nCompose hardening\n'
contains docker-compose.prod.yml 'max-size' "log rotation is configured"
contains docker-compose.prod.yml 'read_only: true' "app filesystem is read-only"
contains docker-compose.prod.yml 'cap_drop:' "capabilities are dropped"
contains docker-compose.prod.yml 'no-new-privileges' "no-new-privileges is set"
contains docker-compose.prod.yml 'stop_grace_period' "graceful stop period is set"

printf '\nnginx\n'
contains deploy/nginx.conf 'ssl_protocols' "TLS protocols are pinned"
contains deploy/nginx.conf 'gzip on' "compression is enabled"
contains deploy/nginx.conf 'location /protected/' "X-Accel-Redirect location exists"
contains deploy/nginx.conf 'internal;' "protected files are internal-only"
contains deploy/security-headers.inc "script-src 'self'" "script-src is self-only"
missing deploy/security-headers.inc "script-src[^;]*unsafe-inline" "script-src has no unsafe-inline"
missing deploy/security-headers.inc "Content-Security-Policy.*unsafe-eval" "CSP has no unsafe-eval"
contains crates/dp-web/src/routes/proof.rs 'x-accel-redirect' "app emits X-Accel-Redirect"
contains crates/dp-web/src/middleware/security.rs 'CONTENT_SECURITY_POLICY' "app sets CSP itself"

printf '\nTemplates (CSP-safe)\n'
if grep -rEq ' on[a-z]+="' crates/dp-web/src/templates 2>/dev/null; then
    fail "templates contain inline event handlers"
else
    pass "no inline event handlers"
fi
if grep -rEq 'hx-on[:=]' crates/dp-web/src/templates 2>/dev/null; then
    fail "templates use hx-on (requires unsafe-eval)"
else
    pass "no hx-on handlers"
fi
contains crates/dp-web/src/templates/base.html 'skip-link' "skip-to-content link exists"
contains crates/dp-web/src/templates/base.html 'id="main-content"' "main landmark is focusable"

printf '\nOperational CLI\n'
contains crates/dp-web/src/main.rs 'Healthcheck' "healthcheck subcommand exists"
contains crates/dp-web/src/main.rs 'with_graceful_shutdown' "server drains on SIGTERM"
contains crates/dp-web/src/main.rs 'DP_LOG_FORMAT' "JSON logging can be enabled"
contains crates/dp-web/src/routes/health.rs 'readyz' "readiness probe exists"

printf '\nBackup and recovery\n'
for script in deploy/backup.sh deploy/restore.sh deploy/restore-drill.sh; do
    exists "$script" "$script exists"
    if [[ -x "$script" ]]; then pass "$script is executable"; else fail "$script is not executable"; fi
done
exists docs/operations/backup-restore.md "backup/restore documentation exists"
exists docs/operations/restore-drill-log.md "restore drill log exists"

printf '\nRunbooks and audits\n'
exists docs/operations/deploy.md "deployment runbook exists"
exists docs/operations/runbook.md "incident runbook exists"
exists docs/operations/logging.md "logging guidance exists"
exists docs/operations/load-test.md "load test procedure exists"
exists docs/operations/a11y-audit.md "accessibility audit exists"
exists docs/operations/launch-checklist.md "launch checklist exists"
exists scripts/loadtest.js "k6 load test exists"

printf '\nReproducible builds\n'
if git ls-files --error-unmatch Cargo.lock >/dev/null 2>&1; then
    pass "Cargo.lock is tracked"
else
    fail "Cargo.lock is not tracked"
fi
if git ls-files --error-unmatch package-lock.json >/dev/null 2>&1; then
    pass "package-lock.json is tracked"
else
    fail "package-lock.json is not tracked"
fi

printf '\n'
if [[ "$failures" -ne 0 ]]; then
    printf '%d hardening check(s) failed\n' "$failures" >&2
    exit 1
fi
printf 'All hardening checks passed.\n'
