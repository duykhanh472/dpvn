# Operations documentation

| Document | Purpose |
|---|---|
| [deploy.md](deploy.md) | First deploy, zero-downtime release, migrations, rollback, TLS |
| [backup-restore.md](backup-restore.md) | Nightly backups, scratch restore, production restore |
| [restore-drill-log.md](restore-drill-log.md) | Performed restore-drill reports |
| [logging.md](logging.md) | JSON logs, `RUST_LOG`, rotation, alerts |
| [load-test.md](load-test.md) | 200-user / 20-saves-per-second k6 procedure |
| [a11y-audit.md](a11y-audit.md) | WCAG 2.2 AA review of the proofing flow |
| [runbook.md](runbook.md) | Incident response: stuck claims, failed import, disk full, DB slow, 5xx |
| [launch-checklist.md](launch-checklist.md) | Production sign-off checklist |

Related files:

- `Dockerfile` — distroless, non-root production image.
- `docker-compose.prod.yml` — TLS stack with read-only containers and log rotation.
- `deploy/nginx.conf` — production TLS/compression/CSP config.
- `deploy/nginx.Dockerfile` — nginx image with baked static assets.
- `deploy/backup.sh`, `deploy/restore.sh`, `deploy/restore-drill.sh`.
- `scripts/loadtest.js` — k6 scenarios.
- `scripts/check-hardening.sh` — static regression checks run in CI.
