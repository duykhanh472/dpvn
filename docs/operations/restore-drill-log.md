# Restore drill log

`deploy/restore-drill.sh` starts a throwaway MySQL 8.4 container, applies the
real migrations, inserts a fixture, runs `deploy/backup.sh`, restores with
`deploy/restore.sh`, and compares per-table checksums. A passing run proves the
Phase 11 acceptance criterion:

> Restore from backup into a clean environment reproduces the site.

Append each run to this file:

```sh
DRILL_LOG=docs/operations/restore-drill-log.md ./deploy/restore-drill.sh
```

| Run | Date | Operator | Result | Notes |
|---|---|---|---|---|
| 1 | 2026-11-01 | Phase 11 implementation | PASS | MySQL 8.4.13, migrations 0001–0003, all table checksums matched |

## Drill run 2026-11-01T02:43:00Z

| Table | Source checksum | Restored checksum | Result |
|---|---|---|---|
| `users` | `3091402494` | `3091402494` | OK |
| `projects` | `2027162975` | `2027162975` | OK |
| `pages` | `2861113387` | `2861113387` | OK |
| `page_rounds` | `4275732555` | `4275732555` | OK |
| `project_events` | `3206910735` | `3206910735` | OK |
| `settings` | `2745902213` | `2745902213` | OK |
