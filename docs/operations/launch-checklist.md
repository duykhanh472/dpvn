# Production launch checklist

Sign off every line before switching DNS to the new deployment. A checked box
means the evidence is attached to the launch ticket (command output, report, or
screenshot).

## 1. Build and packaging

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --all-targets -- -D warnings` passes.
- [ ] `cargo test --workspace` passes against MySQL 8.4.
- [ ] `npm ci && npm run build` succeeds and `static/manifest.json` lists the
      hashed `htmx.*.min.js`, `app.*.js`, `proof.*.js`, and `app.*.css`.
- [ ] `docker build -t dproofreaders:<tag> .` succeeds.
- [ ] Image runs as UID 65532
      (`docker inspect -f '{{.Config.User}}' dproofreaders:<tag>`).
- [ ] `docker history` shows no build secrets or `.env` contents.
- [ ] `./scripts/check-hardening.sh` passes.

## 2. Configuration and secrets

- [ ] `.env` created from `.env.example`; `DB_PASSWORD`, `DB_ROOT_PASSWORD`,
      and `DP_PUBLIC_URL` are set and stored in the secret manager.
- [ ] `config.toml` `[server].public_url` is the HTTPS origin.
- [ ] `[database].max_connections` matches MySQL `max_connections` headroom.
- [ ] SMTP relay reachable; a registration activation email arrives.
- [ ] `dp-data` volume owned by `65532:65532`.
- [ ] TLS certificate and key installed; expiry > 30 days.

## 3. Database

- [ ] `dp migrate` applied cleanly on an empty database.
- [ ] `dp migrate` applied cleanly on a copy of production.
- [ ] `/readyz` returns 200 after migration.
- [ ] Hot indexes present (`pr_pool`, `pages_order`, `pe_project`).
- [ ] `dp verify` passes.

## 4. Backup and recovery

- [ ] Nightly backup cron installed and has run at least once.
- [ ] Backup files have `.sha256` sidecars and pass `gzip -t`.
- [ ] **Restore drill performed** and recorded in
      `docs/operations/restore-drill-log.md` with all checksums `OK`.
- [ ] A scratch restore (`dp_restore`) passes `dp verify` and a login/proofing
      smoke test.
- [ ] Off-host copy configured (restic or equivalent) and tested.
- [ ] Backup-age alert configured (26 h).

## 5. Security

- [ ] CSP has `script-src 'self'` and **no** `unsafe-inline`/`unsafe-eval` for
      scripts (`curl -I https://.../`).
- [ ] HSTS, `nosniff`, `Referrer-Policy`, `frame-ancestors 'none'` present.
- [ ] `/protected/` is `internal` and returns 404 when requested directly.
- [ ] Session cookie is `HttpOnly`, `Secure`, `SameSite=Lax`.
- [ ] Login/registration rate limits active.
- [ ] No secrets in logs (spot-check a login, a save, and an admin action).
- [ ] Dependency audit reviewed (`cargo audit` / `npm audit`); no unpatched
      critical advisories.

## 6. Operations

- [ ] `dp prune` runs every 15 minutes (cron/systemd timer).
- [ ] Log rotation configured (`max-size`/`max-file` or journald).
- [ ] `DP_LOG_FORMAT=json` in production; sample line parsed by the collector.
- [ ] Alerts configured: 5xx > 1%/5 min, p95 save > 500 ms/10 min, pool > 90%,
      disk > 80%, stale-claim job missing 30 min, `readyz` failing twice,
      backup older than 26 h.
- [ ] Runbook reviewed by on-call (`docs/operations/runbook.md`).
- [ ] Rollback procedure rehearsed on staging.

## 7. Performance

- [ ] Load test run at 200 concurrent users / 20 saves per second.
- [ ] No 5xx during the load test.
- [ ] No InnoDB deadlocks during the load test.
- [ ] p95 save latency < 500 ms; p95 browse < 1000 ms.
- [ ] Result recorded in `docs/operations/load-test.md` §5.

## 8. Accessibility and content

- [ ] Manual keyboard walkthrough passed (`docs/operations/a11y-audit.md` §5).
- [ ] Screen-reader spot check passed (NVDA/Firefox, VoiceOver/Safari).
- [ ] Contrast re-measured after any palette change.
- [ ] Error pages are Vietnamese and escaped (`/does-not-exist`, `/admin` while
      logged out, a 429 save).
- [ ] Proofing flow works with `prefers-reduced-motion`.

## 9. Functional smoke test (production)

- [ ] Register → activation email → log in.
- [ ] Create/import a small project; pages appear with images.
- [ ] Claim a page, proof it, save; tally increments.
- [ ] Wordcheck flags a known misspelling; confirm and save.
- [ ] Format preview renders; diff page renders.
- [ ] Comment on a project; comment appears.
- [ ] Admin: grant `pm`, disable a user, toggle `on_hold`, view audit log.
- [ ] Log out; session cookie cleared.
- [ ] `/healthz` and `/readyz` return 200.
- [ ] `/static/...` returns `Cache-Control: public, immutable`.
- [ ] `/protected/images/...` returns 404 to an unauthenticated client.

## 10. Sign-off

| Role | Name | Date | Signature |
|---|---|---|---|
| Engineering | | | |
| Operations / on-call | | | |
| Product owner | | | |
| Accessibility reviewer | | | |

**Launch decision:** ☐ Go  ☐ No-go

Notes / accepted risks:

---

_Last updated: Phase 11._
