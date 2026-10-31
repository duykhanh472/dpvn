# Distributed Proofreaders — Rust Rewrite

**Implementation Plan**

| | |
|---|---|
| Status | Ready to execute |
| Target | Greenfield, single-language Vietnamese proofreading platform |
| Stack | Rust (2024) · axum · sqlx/MySQL · minijinja · HTMX · CodeMirror 6 |
| Deploy | 1 Rust binary + MySQL 8 + nginx |
| Scope | Core proofreading engine (P1–F2), no legacy baggage |
| Estimate | 26 person-weeks (≈6 months solo, ≈3.5 months with 2 devs) |

---

## Table of contents

1. [Summary](#1-summary)
2. [Locked decisions](#2-locked-decisions)
3. [Scope](#3-scope)
4. [Architecture](#4-architecture)
5. [Domain model](#5-domain-model)
6. [Concurrency design](#6-concurrency-design)
7. [Database](#7-database)
8. [Vietnamese text handling](#8-vietnamese-text-handling)
9. [Wordcheck](#9-wordcheck)
10. [Diffing](#10-diffing)
11. [Ingest pipeline](#11-ingest-pipeline)
12. [Auth, sessions, CSRF](#12-auth-sessions-csrf)
13. [Web layer](#13-web-layer)
14. [Proofing interface](#14-proofing-interface)
15. [File storage](#15-file-storage)
16. [CLI and jobs](#16-cli-and-jobs)
17. [Testing strategy](#17-testing-strategy)
18. [Rust engineering standards](#18-rust-engineering-standards)
19. [Phase plan](#19-phase-plan)
20. [Milestones](#20-milestones)
21. [Deployment and operations](#21-deployment-and-operations)
22. [Risks](#22-risks)
23. [Open product decisions](#23-open-product-decisions)
24. [Deferred to v2](#24-deferred-to-v2)
25. [Appendices](#25-appendices)

---

## 1. Summary

A from-scratch proofreading platform for a Vietnamese book community. Volunteers
proof OCR text page-by-page against page images, through ordered rounds
(P1 → P2 → P3 → F1 → F2), with wordcheck assistance and OCR/text diffing.

This is **not** a migration of the upstream Distributed Proofreaders PHP codebase.
That project serves as a reference for domain rules only. No legacy schema, no
legacy data, no legacy auth, no public API, no forum integration.

**What exists at the end:** a proofreader can register, browse available
projects, claim a page, compare text to image, get wordcheck feedback, save, and
watch the project advance to the next round. A project manager can create a
project from uploaded images and OCR text and shepherd it through the rounds.

**Shape of the thing:** ~13 tables, ~15 direct dependencies, one static binary,
one config file, no JavaScript framework.

### By the numbers

| Metric | Value |
|---|---|
| Database tables | 13 |
| Direct crates | 15 |
| Estimated Rust + templates | ~14,000 lines |
| Estimated frontend JS | ~1,200 lines (own code) + vendored CM6/HTMX |
| Phases | 11 |
| Effort | 26 person-weeks |

---

## 2. Locked decisions

| # | Decision | Rationale |
|---|---|---|
| D1 | Greenfield, no legacy import | Independent tool for a different community |
| D2 | Single language: Vietnamese | No i18n layer at all; strings live in templates and `messages.rs` |
| D3 | No public API | Internal server-rendered HTMX app only |
| D4 | Self-contained auth | `argon2id` + own `sessions` table; no phpBB/XenForo |
| D5 | MySQL 8, kept for familiarity | `utf8mb4`, InnoDB, `sqlx` runtime queries |
| D6 | `sqlx` runtime queries only | No `query!` macro, no `cargo sqlx prepare`, no live DB at build time |
| D7 | `minijinja` templates | Hot reload in dev, embedded in release |
| D8 | No Tower middleware stack | One ~30 LOC `AuthUser` extractor; nginx handles HTTP concerns |
| D9 | `lexopt` for CLI | Tiny, no derive macros, one binary |
| D10 | HTMX + CodeMirror 6 | Approved: small libraries, not frameworks |
| D11 | Local filesystem for images | Immutable originals + derivatives, served by nginx |
| D12 | Wordcheck advisory, content-bound confirmation | Highlight + confirm, never a bare boolean |
| D13 | No JS build framework | esbuild only, to bundle CM6 |
| D14 | Rounds are config, not schema | `rounds.toml` compiled in |
| D15 | Diffs computed on request, never stored | `similar` crate replaces ~2,000 legacy lines |

---

## 3. Scope

### 3.1 In scope (v1)

**Identity**
- Registration with email verification
- Login / logout, session management, CSRF
- Roles: `proofer`, `pm`, `pp`, `admin`

**Projects**
- Create project (metadata + charset + tone style)
- Ingest images + OCR text (zip upload, validation, page split)
- State machine: draft → queued → in_round → post_processing → posted → archived
- Project comments (flat thread, HTMX)

**Rounds**
- Configurable rounds P1, P2, P3, F1, F2
- Per-round page pool, atomic claim, auto-release
- Automatic round advancement when a round completes
- Daily page limits per user per round

**Proofing**
- Single-page interface: image pane + CM6 text pane
- Character palette (Vietnamese)
- Wordcheck: phonotactic validation + site/project word lists
- Advisory confirmation flow with content-bound hash
- Save with version history

**Diffing**
- OCR ↔ P1 ↔ P2 word-level diffs, rendered server-side

**Formatting**
- Server-side format preview parser (`_italic_`, `*bold*`, `#` blockquote, poetry, `<sc>`)

**Operations**
- `dp` binary with `serve`, `migrate`, `prune`, `create-admin`, `import-project`
- Docker Compose (app + MySQL + nginx), migration on deploy, backup scripts

### 3.2 Out of scope (v1)

Post-processing tooling · smooth reading · quizzes · tasks · teams · stats charts ·
notifications/digests · export formats (EPUB/HTML) · full-text search ·
accent-insensitive search · i18n · public API · forum · mobile app.

### 3.3 Definition of done for v1

1. A new user registers, verifies email, logs in.
2. A manager imports a 300-page book and queues it.
3. Proofers claim pages concurrently; no page is ever claimed twice.
4. A proofer saves a page, sees wordcheck warnings, confirms, and the tally increments.
5. When the last P1 page is saved, the project moves to P2 and P1 text seeds P2.
6. The diff view shows OCR vs P1 changes.
7. `docker compose up` on a clean machine produces a working site.
8. Restore from backup is tested and documented.

---

## 4. Architecture

### 4.1 Topology

```
                    ┌──────────────────────────────────────┐
   browser ────────▶│ nginx                                │
                    │  · TLS termination                   │
                    │  · gzip/brotli                       │
                    │  · /static/*  → disk (sendfile)      │
                    │  · /protected/* internal (X-Accel)   │
                    │  · everything else → 127.0.0.1:8080  │
                    └──────────────┬───────────────────────┘
                                   │
                    ┌──────────────▼───────────────────────┐
                    │ dp (single Rust binary)              │
                    │  · axum router                       │
                    │  · minijinja templates               │
                    │  · sqlx pool (max 16)                │
                    │  · lexopt subcommands                │
                    └──────┬──────────────────┬────────────┘
                           │                  │
                    ┌──────▼──────┐    ┌──────▼──────────────┐
                    │ MySQL 8     │    │ data/ (local disk)  │
                    │ utf8mb4     │    │  images/ display/   │
                    └─────────────┘    │  tmp/ uploads/      │
                                       └─────────────────────┘
```

No Redis. No message queue. No separate worker process. The `dp` binary serves
HTTP and runs cron jobs as subcommands.

### 4.2 Request lifecycle

```
request
  → axum Router match
  → extractors:
       AuthUser        (parse cookie → sha256 → SELECT session JOIN users)
       CsrfToken       (non-GET only)
       Path/Query/Form (serde)
  → handler:
       load domain objects
       authorize (role + ownership)
       run domain operation in a transaction
       build template context
  → minijinja render
  → response
       full page  (200, text/html)
       HTMX partial (200, text/html fragment)
       redirect   (303 after POST)
       error      (mapped status + error page/partial)
```

Every handler is `async fn(...) -> Result<Response, AppError>`. `AppError`
implements `IntoResponse` so `?` works everywhere.

### 4.3 Workspace layout

```
Cargo.toml                     # [workspace]
crates/
  dp-core/                     # no I/O, no axum, no sqlx
    src/
      lib.rs
      ids.rs                   # UserId, ProjectId, PageId newtypes
      project.rs               # Project, Phase, ToneStyle
      round.rs                 # RoundId, RoundConfig, PageState
      transition.rs            # state machine: rules, guards, apply
      wordcheck/
        mod.rs
        tokenize.rs            # syllable tokenizer (regex)
        phonotactic.rs         # onset/nucleus/coda/tone tables
        lists.rs               # site/project word lists
      diff.rs                  # similar wrapper + HTML renderer
      format_preview/
        mod.rs
        lexer.rs
        parser.rs
      viet.rs                  # NFC, tone stripping, charset
      error.rs                 # thiserror domain errors
  dp-db/
    src/
      lib.rs                   # pool, transaction helpers
      models.rs                # #[derive(FromRow)] structs
      queries/
        users.rs
        sessions.rs
        projects.rs
        pages.rs
        rounds.rs
        words.rs
        comments.rs
        tallies.rs
    migrations/
      0001_init.sql
      0002_seed_words.sql
  dp-web/
    src/
      main.rs                  # lexopt dispatch → serve | migrate | prune | ...
      app.rs                   # Router, AppState
      config.rs                # env + TOML
      auth/
        mod.rs
        extractor.rs           # AuthUser, AdminUser
        argon.rs
        session.rs
        csrf.rs
      routes/
        home.rs
        auth.rs
        projects.rs
        pool.rs
        proof.rs
        wordcheck.rs
        diff.rs
        comments.rs
        admin.rs
      middleware/
        request_id.rs          # ~20 LOC
      templates/               # minijinja
        base.html
        macros.html
        home.html
        auth/{login,register,activate}.html
        projects/{list,detail,form}.html
        proof/{proof.html,_wordcheck.html,_confirm.html}
        admin/*.html
      assets/
        proof.js               # CM6 entry
        app.js                 # HTMX extras, palette
        app.css
  data/                        # runtime, not in repo
tests/
  e2e/                         # playwright
```

`dp-core` is pure: no async, no DB, no filesystem. That is what makes the domain
rules unit-testable and keeps compile times low.

### 4.4 Configuration

Two sources, deliberately separated:

**`config.toml`** — deployment, read once at startup:

```toml
[server]
bind = "127.0.0.1:8080"
public_url = "https://proof.example.org"

[database]
url = "mysql://dp:secret@127.0.0.1:3306/dp"
max_connections = 16
min_connections = 2

[paths]
data = "/var/lib/dp"
templates = "/etc/dp/templates"   # dev only; empty in release = embedded

[mail]
smtp_url = "smtp://localhost:25"
from = "no-reply@proof.example.org"

[limits]
max_upload_mb = 512
daily_pages = 100
claim_timeout_hours = 72
```

**`rounds.toml`** — domain config, compiled into the binary (Appendix A).

Secrets come from environment (`DP_DATABASE_URL`, `DP_SMTP_URL`) and override
the file. Never log the config struct.

### 4.5 Error model

```rust
// dp-core/src/error.rs
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("project {0} not found")]
    ProjectNotFound(i64),
    #[error("page is not available")]
    PageUnavailable,
    #[error("round {round} is not complete")]
    RoundIncomplete { round: String },
    #[error("transition {from} -> {to} is not permitted for {actor}")]
    TransitionDenied { from: String, to: String, actor: String },
    #[error("daily limit of {limit} pages reached")]
    DailyLimitReached { limit: u32 },
}

// dp-web/src/app.rs
#[derive(Debug)]
pub enum AppError {
    Domain(DomainError),
    Db(sqlx::Error),
    Template(minijinja::Error),
    NotFound,
    Forbidden,
    BadRequest(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            AppError::Domain(DomainError::PageUnavailable) => (StatusCode::CONFLICT, ...),
            AppError::Domain(DomainError::DailyLimitReached { .. }) => (StatusCode::TOO_MANY_REQUESTS, ...),
            AppError::Forbidden => (StatusCode::FORBIDDEN, ...),
            AppError::NotFound => (StatusCode::NOT_FOUND, ...),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, ...),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, ...),
        };
        // HTMX requests get an HTML fragment; others get a full page.
        ...
    }
}
```

Rules:
- `thiserror` in `dp-core` and `dp-db`; `AppError` at the web edge.
- No `anyhow` in request paths — it erases the HTTP mapping.
- `anyhow` is allowed in CLI subcommands and one-off scripts.
- Every 5xx logs the full source chain once, with `tracing::error!`.
- Never surface `sqlx::Error` text to the client.

### 4.6 Observability

- `tracing` + `tracing-subscriber` with `EnvFilter`.
- `#[tracing::instrument(skip(state, form), fields(user = %auth.username))]` on handlers.
- One span per request with `request_id`, `method`, `path`, `status`, `duration_ms`.
- Structured fields, not interpolated strings.
- **Never log**: passwords, password hashes, session tokens, CSRF tokens, email
  bodies, full page text.
- `/healthz` returns 200 with DB ping; `/readyz` checks migrations applied.

---

## 5. Domain model

### 5.1 Entities

```
User ──┬── has many Sessions
       ├── has many Roles
       └── proofs Pages

Project ──┬── has many Pages
          ├── has many ProjectEvents
          ├── has many ProjectComments
          ├── has many ProjectWords
          └── has current_round → Round

Page ──┬── belongs to Project
       ├── has one immutable ocr_text + image
       └── has many PageRounds (one per round it has entered)
                     │
                     └── has many PageVersions (history)

Round (config) ── defines charset, order, access threshold
```

### 5.2 Phases

Seven phases replace the upstream's ~20 states. "Available vs in progress" is
**derived from page state**, never stored on the project.

```rust
pub enum Phase { Draft, Queued, InRound, PostProcessing, Posted, Archived }
```

`current_round: Option<RoundId>` is `Some` only while `phase == InRound`.

Derived predicates (computed in SQL or in the view model):

| Predicate | Definition |
|---|---|
| `is_available(round)` | `phase='in_round' AND current_round=:round AND on_hold=0` |
| `is_in_progress` | `phase='in_round'` AND ≥1 page `state='in_progress'` |
| `is_complete_in_round` | `phase='in_round'` AND 0 pages `state != 'done'` |

### 5.3 Transition table

| From | To | Actor | Guard | Action |
|---|---|---|---|---|
| Draft | Queued | Manager, Admin | ≥1 page; every page has an image; every page has OCR text | set `queued_at` |
| Queued | InRound(P1) | Manager, Admin | charset valid; project name/tone_style set | `current_round = first_round` |
| Queued | Draft | Manager, Admin | — | — |
| InRound(Rₙ) | InRound(Rₙ₊₁) | **Auto** | 0 pages in Rₙ with `state != 'done'` | seed Rₙ₊₁ rows from Rₙ text |
| InRound(last) | PostProcessing | **Auto** | 0 pages not done | `current_round = NULL` |
| PostProcessing | Posted | Manager, PP, Admin | — | — |
| Posted | Archived | Admin | — | — |
| InRound | Draft | Admin | — | destructive; requires confirmation |
| (any) | (same) | Manager, Admin | toggle `on_hold` | — |

Automatic transitions are not user actions. They fire inside the page-save
transaction (see §6.2).

### 5.4 Transition implementation

`dp-core` holds the rules as data, not as a pile of closures:

```rust
pub struct Transition {
    pub from: Phase,
    pub to: Phase,
    pub actor: Actor,
    pub guard: Guard,
    pub action: Action,
}

pub enum Actor { Auto, Manager, Pp, Admin, AnyOf(&'static [Actor]) }

pub enum Guard {
    HasPages,
    AllPagesHaveImages,
    AllPagesHaveOcr,
    RoundComplete,
    Never,               // manual-only paths use Never for auto
}

pub enum Action {
    None,
    SetRound(RoundId),
    SeedNextRound,
    ClearRound,
}
```

`fn apply(project, transition, actor) -> Result<Vec<Effect>>` is pure. The web
layer turns `Effect`s into SQL inside the same transaction. This keeps the
state machine testable without a database and makes illegal transitions
impossible to express accidentally.

### 5.5 Rounds

Rounds are ordered and configured in `rounds.toml` (Appendix A). Each round has:

| Field | Meaning |
|---|---|
| `id` | `P1`, `P2`, `P3`, `F1`, `F2` |
| `kind` | `proof` or `format` |
| `charset` | which charset definition applies |
| `min_pages` | pages the user must have proofed before access |
| `enabled` | allow disabling a round without deleting config |

Adding a round is a config edit. No migration.

### 5.6 Daily limits

Computed from `page_tallies` for the current date **in `Asia/Ho_Chi_Minh`**:

```sql
SELECT COALESCE(SUM(count), 0)
FROM page_tallies
WHERE user_id = ? AND round = ? AND day = ?   -- day computed in Asia/Ho_Chi_Minh
```

Checked before claim. The limit is a soft guard against a small race (two tabs
claiming simultaneously could exceed by one); acceptable, documented.

---

## 6. Concurrency design

This section is the highest-risk part of the system. Get it wrong and you
silently corrupt project state.

### 6.1 Invariants

1. A page in a given round is claimed by at most one user.
2. A round advances exactly once.
3. A page save is atomic: text, version, tally, page state, and any project
   transition all commit or none do.
4. A user never proofs their own page from the previous round.

### 6.2 Claiming a page

Two statements, in order. Never `SELECT` then `UPDATE` without a guard.

```sql
-- 1. pick a candidate (read-only, no lock)
SELECT p.id
FROM page_rounds pr
JOIN pages p      ON p.id = pr.page_id
JOIN projects pj  ON pj.id = p.project_id
WHERE pj.id = :project_id
  AND pr.round = :round
  AND pr.state = 'available'
  AND pj.phase = 'in_round'
  AND pj.current_round = :round
  AND pj.on_hold = 0
  AND NOT EXISTS (
        SELECT 1 FROM page_rounds prev
        WHERE prev.page_id = pr.page_id
          AND prev.round = :prev_round
          AND prev.proofer_id = :user_id)
ORDER BY p.page_number
LIMIT 1;

-- 2. claim atomically; the WHERE clause is the lock
UPDATE page_rounds
   SET state = 'in_progress',
       proofer_id = :user_id,
       claimed_at = UTC_TIMESTAMP(6)
 WHERE page_id = :page_id
   AND round = :round
   AND state = 'available';
```

`rows_affected() == 1` means we won. `0` means someone else won — loop back to
step 1 (bounded retries, then return 409).

**Do not** use `SELECT ... FOR UPDATE` here. The conditional `UPDATE` is a
single atomic operation and avoids a lock on the whole pool.

### 6.3 Saving a page — the critical section

```sql
BEGIN;

-- Lock the project first. This is the only serialization point.
SELECT id, phase, current_round, on_hold
  FROM projects
 WHERE id = :project_id
   FOR UPDATE;

-- Verify the page is ours and still in progress.
UPDATE page_rounds
   SET state = 'done',
       text = :text,
       done_at = UTC_TIMESTAMP(6)
 WHERE page_id = :page_id
   AND round = :round
   AND state = 'in_progress'
   AND proofer_id = :user_id;
-- rows_affected must be 1, else ROLLBACK -> 409

INSERT INTO page_versions (page_id, round, text, proofer_id, created_at)
VALUES (:page_id, :round, :text, :user_id, UTC_TIMESTAMP(6));

INSERT INTO page_tallies (user_id, round, day, count)
VALUES (:user_id, :round, :local_day, 1)
ON DUPLICATE KEY UPDATE count = count + 1;

-- Are we the last page in this round?
SELECT COUNT(*) FROM page_rounds
 WHERE round = :round
   AND state <> 'done'
   AND page_id IN (SELECT id FROM pages WHERE project_id = :project_id);

-- If 0 and project.phase = 'in_round' and current_round = :round:
--   UPDATE projects SET phase/current_round ...
--   INSERT INTO project_events ...
--   INSERT INTO page_rounds (...) SELECT ... seeding next round

COMMIT;
```

**Why `FOR UPDATE` on `projects`:** two proofers finishing the last two pages
concurrently would both observe `count = 0` and both advance. Locking the
project row serializes them; the second transaction re-reads `current_round`
after acquiring the lock and (because it changed) does not advance again.

Always re-check `phase` and `current_round` **after** the lock is acquired. The
`SELECT ... FOR UPDATE` result is the authoritative state.

### 6.4 Lock ordering

Fixed global order, to prevent deadlocks:

```
projects  →  pages  →  page_rounds  →  page_versions / page_tallies
```

Every transaction that touches more than one of these follows the order. Never
lock `page_rounds` before `projects`.

### 6.5 Round advancement (seed next round)

```sql
INSERT INTO page_rounds (page_id, round, state, text)
SELECT p.id, :next_round, 'available', pr.text
  FROM pages p
  JOIN page_rounds pr
    ON pr.page_id = p.id
   AND pr.round = :current_round
 WHERE p.project_id = :project_id;
```

Idempotency: `PRIMARY KEY (page_id, round)` makes a double-advance a duplicate-key
error rather than silent duplication. Catch it and treat "next round already
seeded" as success.

### 6.6 Auto-release of stale claims

Run by `dp prune` (cron, every 15 minutes):

```sql
UPDATE page_rounds
   SET state = 'available', proofer_id = NULL, claimed_at = NULL
 WHERE state = 'in_progress'
   AND claimed_at < UTC_TIMESTAMP(6) - INTERVAL :hours HOUR
   AND text IS NULL;   -- never discard work that was actually saved
```

The `text IS NULL` clause is a safety valve: if the proofer saved a draft, keep
the claim until the timeout is much longer.

### 6.7 Concurrency tests (mandatory before Phase 5 is done)

| Test | Setup | Assertion |
|---|---|---|
| double claim | 2 tasks claim the same page | exactly 1 succeeds |
| last-page race | 2 tasks save the last 2 pages of P1 | phase advances exactly once |
| duplicate save | same page saved twice | second returns 409, tally increments once |
| own-page rule | user claims a page they proofed in P1 (now P2) | candidate query excludes it |
| deadlock | 20 concurrent saves across 5 projects | no deadlock, all commit or cleanly retry |

Use `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]` with a real
MySQL container. These are not optional; they are the acceptance criteria.

---

## 7. Database

### 7.1 Conventions

| Convention | Rule |
|---|---|
| Engine | InnoDB |
| Charset | `utf8mb4` |
| Default collation | `utf8mb4_vietnamese_ci` |
| Username/email collation | `utf8mb4_0900_ai_ci` (accent + case insensitive) |
| Primary keys | `BIGINT UNSIGNED AUTO_INCREMENT` |
| Timestamps | `DATETIME(6)`, always UTC (`UTC_TIMESTAMP(6)`) |
| Booleans | `TINYINT(1)` |
| Enums | MySQL `ENUM` for stable small sets; Rust enum is the source of truth |
| Text | `LONGTEXT` for page text, `TEXT` for comments |
| Naming | `snake_case`, singular table names |
| FKs | Explicit, with `ON DELETE` behavior chosen deliberately |
| Migrations | `dp-db/migrations/NNNN_name.sql`, forward-only |

Timezone note: store UTC, compute "today" in `Asia/Ho_Chi_Minh` at the
application boundary. Never store local time.

MySQL variant note: `utf8mb4_0900_ai_ci` is MySQL 8 only. If MariaDB is chosen,
substitute `utf8mb4_general_ci` for the username/email columns. Pin the engine.

### 7.2 Schema

```sql
-- ============================================================
-- 0001_init.sql
-- ============================================================

CREATE TABLE users (
  id             BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  username       VARCHAR(32)  NOT NULL,
  email          VARCHAR(255) NOT NULL,
  password_hash  VARCHAR(255) NOT NULL,
  real_name      VARCHAR(100) NOT NULL DEFAULT '',
  status         ENUM('pending','active','disabled') NOT NULL DEFAULT 'pending',
  activate_token CHAR(43)     NULL,
  created_at     DATETIME(6)  NOT NULL,
  last_login_at  DATETIME(6)  NULL,
  UNIQUE KEY users_username (username),
  UNIQUE KEY users_email    (email),
  KEY users_status (status)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

ALTER TABLE users
  MODIFY username VARCHAR(32)  COLLATE utf8mb4_0900_ai_ci NOT NULL,
  MODIFY email    VARCHAR(255) COLLATE utf8mb4_0900_ai_ci NOT NULL;

CREATE TABLE user_roles (
  user_id BIGINT UNSIGNED NOT NULL,
  role    ENUM('proofer','pm','pp','admin') NOT NULL,
  granted_at DATETIME(6) NOT NULL,
  PRIMARY KEY (user_id, role),
  CONSTRAINT fk_roles_user FOREIGN KEY (user_id)
    REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE sessions (
  token_hash   BINARY(32) PRIMARY KEY,
  user_id      BIGINT UNSIGNED NOT NULL,
  csrf_token   CHAR(43) NOT NULL,
  created_at   DATETIME(6) NOT NULL,
  last_seen_at DATETIME(6) NOT NULL,
  expires_at   DATETIME(6) NOT NULL,
  ip           VARBINARY(16) NULL,
  user_agent   VARCHAR(255) NOT NULL DEFAULT '',
  KEY sessions_user   (user_id),
  KEY sessions_expiry (expires_at),
  CONSTRAINT fk_sess_user FOREIGN KEY (user_id)
    REFERENCES users(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE projects (
  id            BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  name          VARCHAR(255) NOT NULL,
  author        VARCHAR(255) NOT NULL DEFAULT '',
  phase         ENUM('draft','queued','in_round','post_processing','posted','archived')
                  NOT NULL DEFAULT 'draft',
  current_round VARCHAR(8)  NULL,
  on_hold       TINYINT(1)  NOT NULL DEFAULT 0,
  tone_style    ENUM('old','new','either') NOT NULL DEFAULT 'either',
  charset       VARCHAR(32) NOT NULL DEFAULT 'vi',
  manager_id    BIGINT UNSIGNED NOT NULL,
  image_source  VARCHAR(64) NOT NULL DEFAULT '',
  comments      LONGTEXT NOT NULL,
  created_at    DATETIME(6) NOT NULL,
  updated_at    DATETIME(6) NOT NULL,
  KEY projects_pool    (phase, current_round, on_hold),
  KEY projects_manager (manager_id),
  CONSTRAINT fk_proj_manager FOREIGN KEY (manager_id) REFERENCES users(id)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE pages (
  id          BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id  BIGINT UNSIGNED NOT NULL,
  page_number INT NOT NULL,
  image_path  VARCHAR(255) NOT NULL,
  image_width  INT UNSIGNED NOT NULL DEFAULT 0,
  image_height INT UNSIGNED NOT NULL DEFAULT 0,
  ocr_text    LONGTEXT NULL,
  UNIQUE KEY pages_order (project_id, page_number),
  CONSTRAINT fk_pages_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_rounds (
  page_id    BIGINT UNSIGNED NOT NULL,
  round      VARCHAR(8) NOT NULL,
  state      ENUM('available','in_progress','done') NOT NULL DEFAULT 'available',
  text       LONGTEXT NULL,
  proofer_id BIGINT UNSIGNED NULL,
  claimed_at DATETIME(6) NULL,
  done_at    DATETIME(6) NULL,
  PRIMARY KEY (page_id, round),
  KEY pr_pool (round, state),
  KEY pr_proofer (proofer_id, round),
  CONSTRAINT fk_pr_page FOREIGN KEY (page_id)
    REFERENCES pages(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_versions (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  page_id    BIGINT UNSIGNED NOT NULL,
  round      VARCHAR(8) NOT NULL,
  text       LONGTEXT NOT NULL,
  proofer_id BIGINT UNSIGNED NULL,
  created_at DATETIME(6) NOT NULL,
  KEY pv_lookup (page_id, round, created_at),
  CONSTRAINT fk_pv_page FOREIGN KEY (page_id)
    REFERENCES pages(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_events (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id BIGINT UNSIGNED NOT NULL,
  from_phase VARCHAR(24) NULL,
  to_phase   VARCHAR(24) NULL,
  from_round VARCHAR(8)  NULL,
  to_round   VARCHAR(8)  NULL,
  actor_id   BIGINT UNSIGNED NULL,          -- NULL = automatic
  note       VARCHAR(255) NOT NULL DEFAULT '',
  created_at DATETIME(6) NOT NULL,
  KEY pe_project (project_id, created_at),
  CONSTRAINT fk_pe_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE site_words (
  kind     ENUM('good','bad') NOT NULL,
  word     VARCHAR(64) NOT NULL,
  added_by BIGINT UNSIGNED NULL,
  added_at DATETIME(6) NOT NULL,
  PRIMARY KEY (kind, word)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_words (
  project_id BIGINT UNSIGNED NOT NULL,
  kind       ENUM('good','bad') NOT NULL,
  word       VARCHAR(64) NOT NULL,
  added_by   BIGINT UNSIGNED NULL,
  added_at   DATETIME(6) NOT NULL,
  PRIMARY KEY (project_id, kind, word),
  CONSTRAINT fk_pw_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE project_comments (
  id         BIGINT UNSIGNED PRIMARY KEY AUTO_INCREMENT,
  project_id BIGINT UNSIGNED NOT NULL,
  author_id  BIGINT UNSIGNED NOT NULL,
  body_md    TEXT NOT NULL,
  body_html  TEXT NOT NULL,               -- rendered at write time
  created_at DATETIME(6) NOT NULL,
  KEY pc_project (project_id, created_at),
  CONSTRAINT fk_pc_project FOREIGN KEY (project_id)
    REFERENCES projects(id) ON DELETE CASCADE
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE page_tallies (
  user_id BIGINT UNSIGNED NOT NULL,
  round   VARCHAR(8) NOT NULL,
  day     DATE NOT NULL,
  count   INT UNSIGNED NOT NULL DEFAULT 0,
  PRIMARY KEY (user_id, round, day)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;

CREATE TABLE settings (
  k VARCHAR(64) PRIMARY KEY,
  v TEXT NOT NULL,
  updated_at DATETIME(6) NOT NULL
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_vietnamese_ci;
```

Storing `body_html` alongside `body_md` is intentional: markdown rendering is
CPU work and comment threads are read far more often than written. Render once
with `pulldown-cmark`, sanitize with `ammonia`, store both.

### 7.3 Hot queries and expected plans

| Query | Access path |
|---|---|
| available projects in a round | `projects_pool (phase, current_round, on_hold)` |
| candidate page for claim | `pr_pool (round, state)` + `pages_order` |
| remaining pages in round | `pr_pool` + join on `pages.project_id` |
| today's tally | PK `(user_id, round, day)` |
| session lookup | PK `token_hash` |
| project comments | `pc_project (project_id, created_at)` |

Every list query is paginated. No unbounded `SELECT *`.

### 7.4 Migrations

- Forward-only. No down migrations.
- `dp migrate` applies pending migrations at startup (or as a deploy step).
- Migration files are numbered and immutable once merged.
- Schema changes ship as new files; never edit `0001_init.sql` after release.
- CI runs migrations against an empty MySQL, then runs the full test suite.

### 7.5 Seeding

`dp-db/migrations/0002_seed_words.sql` inserts a starter site word list:

- Vietnamese function words and common syllables (~2,000 entries)
- Proper nouns from the community's subject matter
- Punctuation-adjacent tokens that are valid but not phonotactic

Seed data lives in a `words.vi.txt` source file and is generated into the
migration, so it's reviewable as plain text.

---

## 8. Vietnamese text handling

### 8.1 Encoding

Input is **modern UTF-8 only**. No `encoding_rs`, no `chardetng`.

Pipeline, applied once at ingest and again on save:

```rust
pub fn normalize(raw: &str) -> String {
    let s = raw.strip_prefix('\u{FEFF}').unwrap_or(raw);  // BOM
    let s = s.replace("\r\n", "\n").replace('\r', "\n");  // line endings
    s.nfc().collect()                                     // NFC
}
```

Invalid UTF-8 at upload is a hard error with a clear message. Never lossily
convert.

### 8.2 Why NFC

`ế` can be U+1EBF (NFC, 1 codepoint) or `ê` + U+0301 (NFD, 2 codepoints).
Without normalization, word lists, diffing, and equality checks silently fail.
Normalize on write; store only NFC.

Add a DB-level guard: a startup check that samples `pages.ocr_text` and the
`site_words` table and reports any non-NFC content.

### 8.3 Collation

| Column | Collation | Why |
|---|---|---|
| default | `utf8mb4_vietnamese_ci` | correct Vietnamese ordering (`ă` after `a`, `đ` after `d`) |
| `users.username`, `users.email` | `utf8mb4_0900_ai_ci` | accent- and case-insensitive uniqueness; prevents `An`/`Ân` impersonation |
| word lists | `utf8mb4_vietnamese_ci` | exact matching; accents are meaningful |

Word matching is deliberately accent-sensitive. `má` and `mà` are different words.

### 8.4 Tone style

`projects.tone_style`:

- `old` (kiểu cũ): `hòa`, `thúy`
- `new` (kiểu mới): `hoà`, `thuý`
- `either` (default): do not flag

The validator knows both placements and only flags a mismatch when the project
has committed to one style. This is a warning, never a block.

### 8.5 Charset

One charset definition (`vi`) plus an optional extension (`vi-fr` for
colonial-era books with French text). Defined in `charsets.toml`:

```toml
[vi]
letters = "aăâbcdđeêghiklmnoôơpqrstuưvxy"     # + uppercase
punctuation = ".,;:!?…—–-()[]{}«»\"'"
symbols = "§¶*†‡"

[vi-fr]
extends = "vi"
letters_extra = "àâæçéèêëîïôœùûüÿ"
```

The character palette in the proofing UI is generated from this. No 11 legacy
charsuite files.

### 8.6 IME composition

Proofers type with Telex/VNI input methods. Rules:

- Never run wordcheck mid-composition.
- Trigger preview on `compositionend`, debounced 400 ms.
- Autosave (if enabled) pauses during composition.
- CM6 handles `compositionstart`/`compositionend`; surrounding JS must respect
  `view.composing`.

### 8.7 Length semantics

`ế` = 3 bytes, 2 codepoints (NFD) / 1 codepoint (NFC), 1 grapheme.

- Validate lengths with `chars().count()`, never `len()`.
- Truncate display strings on grapheme boundaries (`unicode-segmentation`).
- `VARCHAR(n)` counts characters, so DB limits match `.chars().count()`.

### 8.8 Timezone

`Asia/Ho_Chi_Minh` (UTC+7, no DST). Daily tallies, limits, and "today" labels
use that zone. Storage is UTC.

```rust
fn local_day(now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&Vietnam).date_naive()
}
```

---

## 9. Wordcheck

### 9.1 Tokenization

Vietnamese is syllabic: text is space-separated syllables, not words. The
upstream regex port works unchanged because it never needed lookarounds here.

```rust
// Ported from the upstream pattern; supported natively by the `regex` crate.
static WORD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?u)[\p{L}\p{M}]+(?:['’][\p{L}\p{M}]+)*").unwrap()
});
```

No `fancy-regex` needed for wordcheck. `fancy-regex` is reserved for the handful
of line-ending and indentation patterns elsewhere.

### 9.2 Three signals, in order of confidence

1. **Phonotactic validity** (default signal, ~200 LOC). Validates syllable
   structure. Catches real typos without a dictionary.
2. **Project word lists** (`project_words`): proper nouns, archaic spellings,
   domain terms. `good` suppresses, `bad` flags.
3. **Site word lists** (`site_words`): site-wide good/bad, applied after project
   lists (project wins).

### 9.3 Phonotactic validator

A Vietnamese syllable is:

```
onset (phụ âm đầu) + medial (âm đệm) + nucleus (âm chính) + coda (âm cuối) + tone
```

Implementation: strip the tone and diacritic class, decompose into parts using a
maximal-onset table, then check membership.

```rust
pub fn validate_syllable(word: &str) -> SyllableCheck {
    let Some((tone, base)) = split_tone(word) else { return SyllableCheck::Invalid };
    let Some(parts) = split_structure(&base) else { return SyllableCheck::Invalid };
    if !ONSETS.contains(parts.onset) { return SyllableCheck::Invalid; }
    if !NUCLEI.contains(parts.nucleus) { return SyllableCheck::Invalid; }
    if let Some(coda) = parts.coda {
        if !CODAS.contains(coda) { return SyllableCheck::Invalid; }
        // Checked syllables: stop codas admit only sắc and nặng.
        if matches!(coda, "p" | "t" | "c" | "ch")
            && !matches!(tone, Tone::Sac | Tone::Nang) {
            return SyllableCheck::Invalid;
        }
    }
    SyllableCheck::Valid
}
```

Tables (data in `dp-core/src/wordcheck/phonotactic.rs`, generated from a TOML
source so a native speaker can review it):

| Table | Contents |
|---|---|
| `ONSETS` | `""`, b, c, ch, d, đ, g, gh, gi, h, k, kh, l, m, n, ng, ngh, nh, p, ph, qu, r, s, t, th, tr, v, x |
| `NUCLEI` | a, ă, â, e, ê, i, o, ô, ơ, u, ư, y + diphthongs/triphthongs (ia, iê, ua, uô, ưa, ươ, oa, oe, uy, uya, iêu, yêu, uôi, ươi, ươu, uây, uyê, oai, oay, …) |
| `CODAS` | `""`, c, ch, m, n, ng, nh, p, t |
| `TONES` | ngang, huyền, sắc, hỏi, ngã, nặng |

Special cases handled explicitly: `gi` and `qu` are onsets; `uy` after `q` is a
nucleus without a medial; `i`/`y` spelling varies and both are accepted.

**Calibration requirement:** the tables must be checked against a corpus of at
least 50,000 words from real OCR output. Target false-positive rate < 0.5%.
Because wordcheck is advisory, false positives are tolerable; false negatives
are not. Tune toward flagging rather than silence.

### 9.4 Check result

```rust
pub struct WordIssue {
    pub word: String,
    pub byte_start: usize,   // for CM6 decoration
    pub byte_end: usize,
    pub kind: IssueKind,     // InvalidSyllable | InProjectBadList | InSiteBadList | ToneStyle
    pub suggestion: Option<String>,   // from project good list / edit distance
}
```

Byte offsets, not char offsets, because CM6 positions are UTF-16 and the
conversion happens once at the boundary.

### 9.5 Confirmation flow

Advisory, but **content-bound**. The naive `confirmed=true` flag is trivially
bypassable: check text A, confirm, submit text B.

```
POST /projects/{pid}/pages/{n}/save
  text=<...>
  confirmed_hash=<optional>

1. normalize(text) -> NFC
2. character validation        -> blocking errors?  -> 400, re-render
3. wordcheck(text)
4. issues.is_empty() || confirmed_hash == sha256_hex(text)?
        yes -> commit (see §6.3)
        no  -> 200 + confirm panel
                (lists issues, embeds sha256 of the current text)
```

The confirm panel is an HTMX swap. No server-side state, no extra table,
replay-safe.

Character validation is **blocking**:

| Condition | Action |
|---|---|
| Control characters (except `\n`, `\t`) | Block |
| Codepoint outside the project charset | Block |
| Invalid syllable | Warn + confirm |
| Word in project bad list | Warn + confirm |
| Tone-style mismatch | Warn + confirm |

### 9.6 Performance

- Wordcheck on a 3,000-character page: target < 5 ms.
- Site word lists are loaded once at startup into a `HashSet` behind an `Arc`.
- Project lists are loaded per request (small) or cached in a `moka`/`HashMap`
  keyed by `(project_id, updated_at)` — only if profiling shows it matters.
- Preview endpoint is debounced client-side; server does no caching.

---

## 10. Diffing

### 10.1 Sources

| View | Comparison | Audience |
|---|---|---|
| OCR ↔ P1 | `pages.ocr_text` vs `page_rounds[P1].text` | P2 proofers, managers |
| P1 ↔ P2 | `page_rounds[P1]` vs `[P2]` | P3 proofers |
| Pn ↔ Pn+1 | generic adjacent rounds | formatters |
| OCR ↔ current | `ocr_text` vs current round | managers, PP |

Nothing is stored. Diffs are computed per request.

### 10.2 Algorithm

`similar` crate:

```rust
use similar::{ChangeTag, TextDiff};

pub fn diff_html(old: &str, new: &str) -> String {
    // Vietnamese is space-delimited at syllable level, so word diffing is
    // both accurate and readable.
    let diff = TextDiff::configure()
        .algorithm(similar::Algorithm::Myers)
        .diff_words(old, new);

    let mut out = String::with_capacity(old.len() + new.len());
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Equal => out.push_str(&escape(change.value())),
            ChangeTag::Delete => write!(out, "<del>{}</del>", escape(change.value())).unwrap(),
            ChangeTag::Insert => write!(out, "<ins>{}</ins>", escape(change.value())).unwrap(),
        }
    }
    out
}
```

Cost: a 3,000-character page is microseconds. No caching layer, no stored diffs,
no MediaWiki diff engine port.

### 10.3 Rendering

- Two-column view: old on the left, new on the right, line numbers.
- Word-level `<ins>`/`<del>` inside line-level context.
- Whitespace differences rendered as `·` toggle.
- Sanitize with `ammonia` after building HTML.
- `<del>` styled `text-decoration: line-through; opacity: .6`,
  `<ins>` `background: #d7f5dd`.
- Accessible: `<ins>`/`<del>` carry `aria-label` (added/changed), plus a
  "changes only" filter.

### 10.4 What this replaces

~2,000 lines of `BBDiffFormatter.inc`, `DifferenceEngineWrapper*.inc`, and the
vendored MediaWiki `DiffEngine.php`. Deleted, not ported.

---

## 11. Ingest pipeline

### 11.1 Accepted formats

Both accepted; the manager picks one at upload.

**Format A — paired files in a zip**

```
0001.png  0001.txt
0002.png  0002.txt
...
```

**Format B — images plus one text file with page markers**

```
images/0001.png
images/0002.png
text.txt:
  <<<PAGE:0001>>>
  Nội dung trang một...
  <<<PAGE:0002>>>
  Nội dung trang hai...
```

Both are validated the same way. Format A is preferred for new projects.

### 11.2 Stages

```
1. receive        stream to data/tmp/{uuid}.zip, enforce max_upload_mb
2. inspect        zip entry count <= 5000; uncompressed total <= 4 GB (zip-bomb guard)
3. extract        to data/tmp/{uuid}/ with path traversal checks (no "..", no absolute)
4. classify       by extension; reject unknown types
5. validate image magic bytes, then decode dimensions; reject corrupt files
6. validate text  UTF-8 -> BOM strip -> CRLF->LF -> NFC
7. pair           match image N to text N; report missing/orphaned
8. dry run        return a preview: page count, dimensions, first 20 lines,
                  errors, warnings
9. commit         copy images into data/images/{project_id}/ via rename()
                  insert pages + initial page_rounds rows in one transaction
10. derive        generate display derivatives (background task)
11. cleanup       remove data/tmp/{uuid}/
```

Stage 8 is mandatory. A manager must see the split before it becomes a project.

### 11.3 Validation rules

| Check | Severity |
|---|---|
| Not a zip | error |
| Entry count > 5,000 | error |
| Uncompressed size > 4 GB | error |
| Path traversal in entry name | error |
| Unsupported file extension | error |
| Image fails magic-byte check | error |
| Image dimensions < 200×200 or > 10,000×10,000 | error |
| Text file not valid UTF-8 | error |
| Page number gap | warning |
| Orphaned image with no text | warning |
| Empty page text | warning |
| Text > 200,000 characters on one page | warning |

Errors block commit. Warnings are shown and require acknowledgment.

### 11.4 Image processing

- Originals stored verbatim (never re-encoded) at
  `data/images/{project_id}/{page_id}.{ext}`.
- Display derivative: longest edge ≤ 1600 px, JPEG q=82, at
  `data/display/{project_id}/{page_id}.jpg`.
- Generated with the `image` crate (features `png`, `jpeg`) inside
  `tokio::task::spawn_blocking`.
- Concurrency capped with a `Semaphore(2)` — image encoding is CPU-bound and
  would otherwise starve request handling.
- If derivative generation fails, the original is still served. Degrade, don't
  fail the import.

### 11.5 Atomicity

- Write to `data/tmp/`, `fsync`, then `rename()` into the final path (same
  filesystem, atomic).
- DB insert is a single transaction. If it fails, delete the copied files.
- A crash between file copy and DB commit leaves orphaned files, never orphaned
  rows. `dp prune` removes files with no DB row older than 24 hours.

### 11.6 Reprocessing

`dp import-project --replace` re-runs ingest into a `draft` project, deleting
existing pages first. Only allowed while `phase = 'draft'`.

---

## 12. Auth, sessions, CSRF

### 12.1 Password hashing

`argon2id`:

| Parameter | Value |
|---|---|
| variant | `Argon2id` |
| memory | 19,456 KiB (19 MiB) |
| iterations | 2 |
| parallelism | 1 |
| output | 32 bytes |

OWASP baseline. Login cost is ~50–100 ms; run inside `spawn_blocking` so the
async runtime is never blocked. Registration and password change have the same
cost — rate-limit them.

Password policy: minimum 10 characters, no maximum below 128, no composition
rules, checked against a small list of the 10,000 most common passwords
(shipped as a file, loaded at startup).

### 12.2 Sessions

```rust
// Issue
let token: [u8; 32] = rand::random();            // OS CSPRNG
let hash = Sha256::digest(token);
INSERT INTO sessions (token_hash, user_id, csrf_token, created_at, last_seen_at, expires_at, ...)
    VALUES (?hash, ?user_id, ?csrf, NOW(6), NOW(6), NOW(6) + INTERVAL 30 DAY)

// Cookie
dp_session=<base64url(token)>; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=2592000
```

| Property | Value |
|---|---|
| Token | 32 bytes from `getrandom`, base64url (43 chars) |
| Stored | `sha256(token)` in `BINARY(32)` — DB leak does not grant sessions |
| Comparison | constant-time (`subtle`) |
| Expiry | absolute 30 days; idle 14 days |
| `last_seen_at` | updated at most once per 5 minutes per session |
| Rotation | new token on privilege change (login, role grant) |
| Logout | `DELETE` row + expire cookie |

### 12.3 Session lookup (the extractor, ~30 LOC)

```rust
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let Some(cookie) = read_cookie(&parts.headers, "dp_session") else {
            return Err(AppError::Unauthorized);
        };
        let hash = Sha256::digest(cookie.as_bytes());
        let row = sqlx::query_as::<_, SessionUser>(SESSION_LOOKUP)
            .bind(hash.as_slice())
            .fetch_optional(&state.db)
            .await?;
        row.map(AuthUser::from).ok_or(AppError::Unauthorized)
    }
}
```

`AdminUser` is a second extractor that wraps `AuthUser` and checks a role. Both
are pure DB lookups, no middleware, no `tower-sessions`.

### 12.4 CSRF

- Per-session `csrf_token` (43 chars), issued at login.
- Exposed in `<meta name="csrf-token" content="...">`.
- HTMX sends it on every request: `hx-headers='{"X-CSRF-Token": "..."}'`.
- Verified on every non-`GET`/`HEAD` request.
- Constant-time comparison.
- Rejected with `403`.

`SameSite=Lax` is a second layer, not a substitute — some flows need POST from
an external redirect.

### 12.5 Registration and activation

1. `POST /register` → validate → create `status='pending'` + `activate_token`
2. Send email with `https://.../activate/{token}`
3. `GET /activate/{token}` → `status='active'`, clear token
4. Unactivated accounts pruned after 7 days by `dp prune`

Rate limits (in-process token bucket, no crate, single instance):
- registration: 5 per hour per IP
- login: 10 per 15 minutes per IP, 5 per username
- password reset: 3 per hour per email

Note: in-process limits assume one instance. Documented; revisit if horizontally
scaled.

### 12.6 Authorization matrix

| Capability | proofer | pm | pp | admin |
|---|---|---|---|---|
| Proof a page | ✓ | ✓ | ✓ | ✓ |
| Comment on project | ✓ | ✓ | ✓ | ✓ |
| Create project | | ✓ | | ✓ |
| Import pages | | own | | ✓ |
| Queue / advance project | | own | | ✓ |
| Mark posted | | own | ✓ | ✓ |
| Archive | | | | ✓ |
| Manage users/roles | | | | ✓ |
| Toggle `on_hold` | | own | | ✓ |

Ownership checks are explicit in handlers, not implicit in the role. Never rely
on the UI hiding a button.

---

## 13. Web layer

### 13.1 Routes

| Method | Path | Auth | Purpose |
|---|---|---|---|
| GET | `/` | — | landing / login |
| GET/POST | `/register` | — | registration |
| GET | `/activate/{token}` | — | email activation |
| GET/POST | `/login` | — | login |
| POST | `/logout` | user | logout |
| GET | `/projects` | user | available projects (filterable) |
| GET | `/projects/{id}` | user | project detail + comments |
| POST | `/projects/{id}/comments` | user | add comment (HTMX) |
| GET | `/projects/{id}/proof` | user | next page → redirect |
| GET | `/projects/{id}/pages/{n}` | user | proofing UI |
| POST | `/projects/{id}/pages/{n}/claim` | user | claim page |
| POST | `/projects/{id}/pages/{n}/save` | user | save text |
| POST | `/projects/{id}/pages/{n}/wordcheck` | user | live issue list (HTMX) |
| GET | `/projects/{id}/pages/{n}/image` | user | image via X-Accel-Redirect |
| GET | `/projects/{id}/pages/{n}/diff/{from}/{to}` | user | diff view |
| GET | `/my/projects` | pm | managed projects |
| GET/POST | `/projects/new` | pm | create project |
| POST | `/projects/{id}/import` | pm | ingest upload |
| POST | `/projects/{id}/transition` | pm | state change |
| GET/POST | `/admin/users` | admin | user management |
| GET/POST | `/admin/projects` | admin | project oversight |
| GET | `/healthz`, `/readyz` | — | probes |

No `/api/v1`. No versioning. HTMX endpoints return fragments from the same
handlers, distinguished by `HX-Request`.

### 13.2 Extractors

| Extractor | Purpose |
|---|---|
| `AuthUser` | session → user; 401 if absent |
| `AdminUser` | `AuthUser` + admin role; 403 otherwise |
| `ManagerUser` | `AuthUser` + pm or admin |
| `Csrf` | validates `X-CSRF-Token` on non-GET |
| `ProjectPath` | `{id}` → `ProjectId` newtype, 404 if absent |
| `Pagination` | `?page=&per=` with sane caps (max 100) |

### 13.3 Templates

```
templates/
  base.html              <html lang="vi">, meta csrf, css, htmx, proof.js
  macros.html            csrf_field(), pagination(), flash()
  home.html
  auth/login.html
  auth/register.html
  auth/activate.html
  projects/list.html
  projects/detail.html
  projects/form.html
  proof/proof.html
  proof/_wordcheck.html      HTMX fragment
  proof/_confirm.html        HTMX fragment
  admin/users.html
  admin/projects.html
  error.html
```

`minijinja` setup:

```rust
let mut env = Environment::new();
env.set_auto_reload(cfg.dev_mode)?;                 // path_loader in dev
env.set_loader(if cfg.dev_mode {
    minijinja::path_loader(&cfg.template_dir)
} else {
    minijinja_embed::load_templates!()               // embedded in release
});
env.set_undefined_behavior(UndefinedBehavior::Strict);  // fail loud, not blank
```

`Strict` undefined behavior converts "typo in a variable name" from a silently
blank page into a 500 in development and a test failure in CI.

**Compile-time checking is gone, so replace it with a test:**

```rust
#[test]
fn all_templates_render() {
    let env = test_env();
    for name in env.template_names() {
        // Render with a fixture context; every template must succeed.
        render_with_fixtures(&env, name).unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}
```

### 13.4 HTMX patterns

| Interaction | Pattern |
|---|---|
| Comment post | `hx-post` → returns the new comment partial → `hx-swap="beforeend"` |
| Project filter | `hx-get` with `hx-trigger="change delay:300ms"` → swaps the list |
| Pagination | `hx-get` → `hx-swap="outerHTML"` on the table |
| Wordcheck preview | `hx-post` on `compositionend`+debounce → swaps the issue panel |
| Save confirmation | save returns `_confirm.html`; the confirm button posts again with `confirmed_hash` |
| Transition | `hx-post` → returns the updated header partial |

**Never** use HTMX for the text editor. CM6 owns that surface and communicates
with `fetch`, not `hx-*`.

### 13.5 Frontend build

```
assets/proof.js  ──esbuild──▶  static/js/proof.<hash>.js
assets/app.js    ──esbuild──▶  static/js/app.<hash>.js
assets/app.css   ──lightningcss──▶  static/css/app.<hash>.css
```

- esbuild only. No webpack, no bundler config beyond a 20-line build script.
- Content-hashed filenames; a `manifest.json` maps logical names to hashed paths.
- HTMX vendored, pinned, served from `static/js/htmx.min.js` (no CDN — CSP and
  offline reliability).
- Target: `es2020`, no polyfills beyond `browserslist` `["> 0.5%", "not dead"]`.

### 13.6 CSS

- One `app.css`, custom properties for theming.
- Logical properties (`margin-inline`, `padding-block`) throughout.
- `line-height: 1.6` minimum in the proofing pane — Vietnamese stacked
  diacritics (`ế`, `ộ`, `ẫ`) clip at 1.2.
- System font stack; no webfont required for Vietnamese in modern OSes.
- Dark mode via `prefers-color-scheme` and a `[data-theme]` override.
- No utility framework.

---

## 14. Proofing interface

The single most important screen. Upstream used an HTML `<frameset>` with
jQuery; this replaces it with one page.

### 14.1 Layout

```
┌──────────────────────────────────────────────────────────────┐
│ toolbar: project · page 12/300 · round P1 · [Save] [Diff]    │
├───────────────────────────┬──────────────────────────────────┤
│                           │                                  │
│         image pane        │        text pane (CM6)           │
│      (pan / zoom /        │                                  │
│       rotate / fit)       │         Nội dung trang...        │
│                           │                                  │
│                           │                                  │
├───────────────────────────┴──────────────────────────────────┤
│ status bar: wordcheck (3) · chars 412 · tally 45/100         │
└──────────────────────────────────────────────────────────────┘
```

Split is draggable, percentage-based, persisted in `localStorage`. Vertical
stacking below 900 px.

### 14.2 CM6 integration

```js
import { EditorView, keymap, lineNumbers, highlightActiveLine } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { history, defaultKeymap, historyKeymap } from "@codemirror/commands";
import { search, searchKeymap } from "@codemirror/search";

const view = new EditorView({
  state: EditorState.create({
    doc: initialText,
    extensions: [
      lineNumbers(),
      history(),
      search(),
      EditorView.lineWrapping,
      EditorView.contentAttributes.of({ spellcheck: "false", lang: "vi" }),
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, saveKeymap]),
      wordcheckDecorations(),
    ],
  }),
  parent: document.getElementById("text-pane"),
});
```

| Concern | Approach |
|---|---|
| Bad-word highlight | `StateField<DecorationSet>` rebuilt from server `WordIssue[]` |
| Invalid codepoint | separate mark decoration, red underline |
| Jump to next issue | `view.dispatch({ selection, effects: scrollIntoView })` |
| Character palette | button inserts at cursor, replacing selection |
| Save | `Ctrl/Cmd+S` → `fetch` POST, then navigate to next page |
| Unsaved warning | `beforeunload` when `docChanged` |
| Composition | suspend decoration updates while `view.composing` |
| Server truth | decorations are advisory only; the save endpoint re-checks |

Coordinates: the server returns **byte offsets**; JS converts to UTF-16 code
units once, using the text it just posted (so offsets always agree).

### 14.3 Page flow

```
GET /projects/{id}/proof
  → find next available page for this user
      none available → redirect to project page with a message
  → 303 to /projects/{id}/pages/{n}

GET /projects/{id}/pages/{n}
  → if page not in_progress or not mine → claim it (idempotent) or 409
  → render proof.html with:
       image URL, page number, total, round
       text = page_rounds.text (seeded from previous round)
       wordcheck issues
       charset palette
```

Claim is an explicit POST from the proofing page's load, not a side effect of
GET. GET must be safe and idempotent.

### 14.4 Save and continue

```
POST /save  → commit (§6.3)
            → if project advanced: response includes the new round
            → 303 to /projects/{id}/proof   (next page)
```

HTMX handles the confirm-panel round trip; the final navigation is a normal
redirect so the URL and history stay honest.

### 14.5 Accessibility

- Toolbar buttons are real `<button>`s with `aria-label` in Vietnamese.
- Palette is keyboard reachable (`Tab`, arrow keys, `Enter`).
- Split handle is a `role="separator"` with `aria-orientation` and arrow-key
  resize.
- Image pane has a text alternative (page number + dimensions).
- Focus ring never removed.
- Tested at 200% zoom and with VoiceOver/NVDA on the toolbar and palette.

---

## 15. File storage

### 15.1 Layout

```
/var/lib/dp/
  images/{project_id}/{page_id}.png      # original, immutable
  display/{project_id}/{page_id}.jpg     # derivative, regenerable
  tmp/                                    # uploads and extraction, cleared on boot
```

`project_id` is a small integer, so each directory holds at most a few thousand
entries. No hashing, no sharding needed.

### 15.2 Serving

Rust authorizes, nginx serves:

```rust
async fn image(auth: AuthUser, path: ProjectPath, page: Path<u32>) -> Result<Response, AppError> {
    authorize_page_view(&auth, path.id, page.0).await?;
    let url = format!("/protected/images/{}/{}.png", path.id.0, page.0);
    Ok(([(header::HeaderName::from_static("x-accel-redirect"), url)], StatusCode::OK).into_response())
}
```

nginx:

```nginx
location /protected/ {
    internal;
    alias /var/lib/dp/;
    add_header Cache-Control "private, max-age=86400";
    add_header X-Content-Type-Options nosniff;
}
```

Benefits: `sendfile`, range requests, caching, and zero image bytes through the
Rust process. Authorization lives in exactly one place.

### 15.3 Safety

- Paths are built from integer IDs only. Never from uploaded filenames.
- Extension is chosen by the server from the validated magic bytes.
- `nosniff` on all served files.
- Directory permissions `0750`, owned by the service user.

### 15.4 Capacity

| Item | Estimate |
|---|---|
| Original PNG, 1500×2200 grayscale | 3–5 MB/page |
| Display JPEG | 150–300 KB/page |
| 300-page book | 1–1.5 GB |
| 100 books in flight | 100–150 GB |

Plan the volume at 4× steady state. Archive `images/` for `posted` projects to
cold storage; keep `display/` (small) hot.

### 15.5 Backup

```sh
# nightly
mysqldump --single-transaction --quick dp | gzip > /backup/db-$(date +%F).sql.gz
restic backup /var/lib/dp/images /var/lib/dp/display
```

- Images are immutable after ingest, so restic incrementals are cheap.
- `display/` is regenerable — exclude it if space-constrained.
- Restore drill is part of Phase 11 acceptance.

---

## 16. CLI and jobs

### 16.1 Subcommands

| Command | Purpose |
|---|---|
| `dp serve` | run the HTTP server |
| `dp migrate` | apply pending migrations |
| `dp prune` | run all maintenance jobs |
| `dp create-admin <username>` | bootstrap the first admin |
| `dp import-project <zip> --name ...` | CLI ingest (same pipeline as web) |
| `dp verify` | check DB/template/file consistency |

`lexopt` dispatch, ~80 lines total. No subcommand frameworks.

### 16.2 Scheduled jobs

Run every 15 minutes from cron or systemd timer:

| Job | Action |
|---|---|
| `release_stale_claims` | §6.6 |
| `expire_sessions` | `DELETE FROM sessions WHERE expires_at < NOW()` |
| `prune_pending_users` | delete `pending` older than 7 days |
| `prune_orphan_files` | delete files with no DB row, older than 24 h |
| `prune_versions` | keep the newest 20 versions per `(page, round)` |
| `prune_events` | keep `project_events` for 2 years |

Idempotent, safe to run concurrently, logged with counts.

### 16.3 Migrations on deploy

```
1. stop accepting traffic (or use a rolling restart)
2. dp migrate
3. restart dp serve
4. curl /readyz
```

Migrations are forward-only and must be backward-compatible with the previous
binary for one release (add columns with defaults; drop columns one release
later).

---

## 17. Testing strategy

### 17.1 Pyramid

| Layer | Tool | Coverage target |
|---|---|---|
| Unit (`dp-core`) | `cargo test` | 90% of domain logic |
| Integration (`dp-db`) | `sqlx::test` + MySQL container | every query used in a handler |
| Concurrency | `#[tokio::test]` + real MySQL | §6.7 table |
| Property | `proptest` | tokenizer, normalizer, diff round-trip |
| Snapshot | `insta` | rendered HTML for key pages |
| Template | render-all-templates test | 100% of templates |
| E2E | Playwright | 12 critical flows |
| Load | `oha` / k6 | 200 concurrent proofers |
| Fuzz | `cargo-fuzz` | ingest splitter, zip parser |

### 17.2 Must-have unit tests

| Module | Cases |
|---|---|
| `viet::normalize` | NFC idempotent; BOM stripped; CRLF→LF; invalid UTF-8 rejected |
| `wordcheck::tokenize` | punctuation, apostrophes, bracketed chars, CJK passthrough, empty |
| `wordcheck::phonotactic` | every onset/nucleus/coda; stop-coda tone rule; `gi`/`qu`; corpus FP rate |
| `transition::apply` | all legal transitions; all illegal ones rejected; actor rules |
| `diff::diff_html` | empty, identical, insert-only, delete-only, escape injection |
| `format_preview` | nested emphasis, unclosed markers, poetry indentation, `<sc>` |

### 17.3 Property tests

```rust
proptest! {
    // Tokenizing then reassembling with the captured separators
    // must reproduce the input exactly.
    #[test]
    fn tokenizer_roundtrip(s in "\\PC{0,500}") {
        let tokens = tokenize(&s);
        prop_assert_eq!(reassemble(&tokens), s);
    }

    // Normalization must be idempotent.
    #[test]
    fn nfc_idempotent(s in "\\PC{0,500}") {
        let once = normalize(&s);
        prop_assert_eq!(normalize(&once), once);
    }
}
```

### 17.4 HTML snapshot tests

Snapshot the full rendered output of: project list, project detail, proof page
(head only), wordcheck fragment, confirm fragment, diff view. Any accidental
markup change shows up as a diff in review.

### 17.5 E2E critical flows

1. Register → activate → login
2. Create project → upload zip → preview → commit
3. Queue project → claim page → proof → save
4. Wordcheck warning → confirm → saved
5. Complete all pages of P1 → project advances to P2
6. Claim exclusion: cannot get own page from previous round
7. Stale claim released then re-claimed by another user
8. Comment post and render
9. Diff view renders correct insertions/deletions
10. Daily limit reached → claim blocked with a clear message
11. Non-manager cannot transition a project
12. Logout invalidates session

### 17.6 Performance targets

| Operation | Target (p95) |
|---|---|
| Project list | < 80 ms |
| Proof page render | < 150 ms |
| Save page | < 200 ms |
| Wordcheck (3k chars) | < 10 ms |
| Diff (3k chars) | < 20 ms |
| Image (via nginx) | < 50 ms TTFB on LAN |

Load test: 200 concurrent users, 20 saves/sec sustained, no deadlocks, no 5xx.

---

## 18. Rust engineering standards

Derived from the project's Rust rule set. These are review criteria, not
suggestions.

### 18.1 Structure

- Workspace with three crates: `dp-core` (pure), `dp-db` (SQL), `dp-web` (HTTP).
- `dp-core` has **no** `tokio`, `sqlx`, `axum`, or filesystem dependencies.
- Modules organized by feature, not by type.
- `pub(crate)` by default; `pub` only across crate boundaries.

### 18.2 Types

- Newtype every ID: `UserId(u64)`, `ProjectId(u64)`, `PageId(u64)`.
- Enums for states and kinds; never stringly-typed domain values.
- Parse at boundaries: `ToneStyle::from_str` returns `Result`, not a guess.
- `Option<T>` for absence; never sentinel values (`0`, `""`).
- `#[non_exhaustive]` on public enums that may grow.

### 18.3 Errors

- `thiserror` for domain and DB errors; `AppError` at the edge.
- No `unwrap()` in production paths. `expect()` only for proven invariants.
- Error messages lowercase, no trailing punctuation, no user-facing text in
  `dp-core` (that belongs in `messages.rs`).
- Never `panic!` on user input.

### 18.4 Ownership

- Borrow over clone; `&str`/`&[T]` in signatures.
- `Cow<'_, str>` when a transformation is usually a no-op (normalization).
- `Arc` for shared read-only config and word lists.
- No `.clone()` to satisfy the borrow checker without a comment explaining why.

### 18.5 Async

- Never hold a lock across `.await` — including DB transactions on a shared
  connection.
- `spawn_blocking` for argon2, image encoding, and zip extraction.
- Bounded work: `Semaphore` around image processing.
- Cancellation-safe futures only in `select!`.
- Pool acquisition timeouts configured; a stalled pool returns 503, not hangs.

### 18.6 SQL

- All queries are `sqlx::query_as::<_, T>()` with `#[derive(FromRow)]`.
- Explicit column lists. `SELECT *` is banned.
- Bind parameters only. No string interpolation into SQL, ever.
- Transactions for anything touching more than one table.
- `#[sqlx(rename = "...")]` where legacy-style column names require it.

### 18.7 Observability

- `tracing` spans on every handler with structured fields.
- Log an error exactly once, with its source chain.
- No secrets, tokens, passwords, or page text in logs.

### 18.8 Lints

```toml
[lints.rust]
unsafe_code = "forbid"
unused_must_use = "deny"

[lints.clippy]
correctness = { level = "deny" }
suspicious  = { level = "warn" }
style       = { level = "warn" }
complexity  = { level = "warn" }
perf        = { level = "warn" }
```

CI: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`.

### 18.9 Build profile

```toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true

[profile.dev.package."*"]
opt-level = 3        # fast deps in dev, fast rebuilds for workspace code
```

Declare `rust-version` in `Cargo.toml` and test it in CI.

### 18.10 Documentation

- `//!` on every module explaining its role.
- `///` on all public items.
- `# Errors` sections on fallible functions.
- Document *why* for any non-obvious concurrency decision — especially §6.

---

## 19. Phase plan

Each phase ends with a working, tested increment. No phase is "80% done".

---

### Phase 1 — Foundation (2 weeks)

**Goal:** a running binary with a schema and a health check.

**Tasks**
- [ ] Cargo workspace, three crates, `rust-version`, release profile, lints
- [ ] `config.toml` + env override + validation at startup
- [ ] `dp-db`: pool setup, `0001_init.sql`, migration runner
- [ ] `dp-core`: ID newtypes, `Phase`, `RoundId`, `ToneStyle`, `DomainError`
- [ ] `dp-web`: axum app skeleton, `AppState`, `AppError` + `IntoResponse`
- [ ] `request_id` middleware (~20 LOC) and `tracing` setup
- [ ] `/healthz`, `/readyz`
- [ ] `minijinja` environment with `Strict` undefined behavior + render-all test
- [ ] `base.html` with `lang="vi"`, CSRF meta, asset manifest
- [ ] esbuild build script producing hashed assets
- [ ] Docker Compose: app, MySQL 8, nginx
- [ ] CI: fmt, clippy, test, migrate-on-empty-DB

**Deliverable:** `docker compose up` → `/healthz` returns 200.

**Acceptance**
- `cargo clippy -- -D warnings` clean
- Migration applies to an empty MySQL
- All templates render in the test suite
- `dp serve` starts with a bad config and exits with a clear error

---

### Phase 2 — Auth (2 weeks)

**Goal:** register, activate, log in, log out.

**Tasks**
- [ ] `users`, `user_roles`, `sessions` queries
- [ ] argon2id hashing + verification in `spawn_blocking`
- [ ] Registration form + validation (username charset, email, password policy)
- [ ] Activation token + email via `lettre`
- [ ] Login, logout, session issue/rotate/expire
- [ ] `AuthUser`, `AdminUser`, `ManagerUser` extractors
- [ ] CSRF token generation + `Csrf` extractor
- [ ] Rate limiting (in-process token bucket)
- [ ] `dp create-admin`
- [ ] `dp prune`: expired sessions, pending users

**Deliverable:** a user can register, verify, log in, and reach an empty dashboard.

**Acceptance**
- Session cookie is `HttpOnly`, `Secure`, `SameSite=Lax`
- Token is not stored in plaintext
- CSRF rejects a POST without the header
- Login is rate-limited
- Passwords never appear in logs

---

### Phase 3 — Ingest (3.5 weeks)

**Goal:** turn a zip into a draft project with pages.

**Tasks**
- [ ] Upload form + streaming to `data/tmp/`
- [ ] Zip inspection: entry count, uncompressed size, path traversal
- [ ] Image magic-byte + dimension validation (`png`, `jpeg-decoder`)
- [ ] Text validation: UTF-8, BOM, CRLF, NFC
- [ ] Format A pairing and Format B marker splitting
- [ ] Dry-run preview page (counts, errors, warnings, first lines)
- [ ] Commit: atomic file copy + transactional insert
- [ ] Display derivative generation (`image`, `spawn_blocking`, `Semaphore(2)`)
- [ ] Project create/edit forms (metadata, tone style, charset, comments)
- [ ] `dp import-project` CLI using the same pipeline
- [ ] `dp prune`: orphan files, tmp cleanup

**Deliverable:** import a real 300-page book and browse it in `draft`.

**Acceptance**
- A corrupt image is rejected with the filename in the message
- A page-number gap produces a warning requiring acknowledgment
- No partially-imported project can exist
- Ingest of 300 pages completes in < 60 s on a laptop
- Re-running with `--replace` is clean

---

### Phase 4 — State machine and pools (2 weeks)

**Goal:** projects move through rounds; pages are claimable atomically.

**Tasks**
- [ ] `rounds.toml` parsing and validation (ordering, unique IDs, first/last)
- [ ] `Transition` table and pure `apply()` in `dp-core`
- [ ] `dp-db` effects executor (SQL per `Effect`)
- [ ] `projects`, `pages`, `page_rounds` pool queries
- [ ] Claim with conditional `UPDATE` + retry
- [ ] Save transaction per §6.3, with `FOR UPDATE` and post-lock re-check
- [ ] Round seeding on advance, idempotent via primary key
- [ ] `page_tallies` increment + daily limit check in `Asia/Ho_Chi_Minh`
- [ ] `project_events` audit writes
- [ ] `dp prune`: stale claim release
- [ ] **All concurrency tests from §6.7**

**Deliverable:** scripted API-level run of a project from `draft` through P1 to P2.

**Acceptance**
- §6.7 concurrency suite passes 100 iterations without flake
- Illegal transitions return 409, never 500
- Daily limit is enforced and resets at local midnight
- Audit trail shows `from → to`, actor, and timestamp for every move

---

### Phase 5 — Proofing interface (5 weeks)

**Goal:** a human can proof a page end-to-end.

**Tasks**
- [ ] `proof.html` layout with draggable split
- [ ] Image pane: fit, zoom, pan, rotate, keyboard shortcuts, preload next
- [ ] CM6 integration: line wrap, history, search, custom keymap
- [ ] Load text, save via `fetch`, unsaved-changes guard
- [ ] Character palette from `charsets.toml`
- [ ] Wordcheck decoration layer (byte → UTF-16 offset conversion)
- [ ] Jump-to-next-issue
- [ ] Issue panel (HTMX) and confirm flow (§9.5)
- [ ] Tally/status bar
- [ ] Next-page navigation, empty-pool message
- [ ] IME composition handling
- [ ] Keyboard-only and screen-reader pass

**Deliverable:** demo a proofer clearing 10 pages; the project advances.

**Acceptance**
- p95 proof-page render < 150 ms
- Saving is idempotent (double-click does not double-count)
- Typing with Telex does not flicker highlights mid-composition
- Works at 200% zoom
- Only the page's own text is editable; the image is not downloadable via a
  guessed URL without authorization

---

### Phase 6 — Wordcheck (2.5 weeks)

**Goal:** useful, calibrated, trustworthy feedback.

**Tasks**
- [ ] Syllable tokenizer + property tests
- [ ] `charsets.toml`, tone splitting, diacritic tables
- [ ] Phonotactic validator + table generation from TOML
- [ ] Corpus calibration (≥50k words from real OCR); tune tables
- [ ] Tone-style checking (`old` / `new` / `either`)
- [ ] `site_words` / `project_words` queries and admin UI
- [ ] Suggestion generation (edit distance ≤ 2 over project good list)
- [ ] Content-bound confirmation flow on the server
- [ ] Seed word list migration

**Deliverable:** a proofer sees meaningful warnings on a real page.

**Acceptance**
- False-positive rate < 0.5% on the calibration corpus
- `nghỉ` vs `nghĩ` style errors are caught
- Confirming text A then submitting text B is rejected
- Wordcheck < 10 ms on 3k characters

---

### Phase 7 — Diffing (1.5 weeks)

**Goal:** compare OCR and round texts.

**Tasks**
- [x] `diff_html` with `similar`, word-level, escaped
- [x] Two-column render template with line context
- [x] `?changes_only=1`, whitespace toggle
- [x] Route + authorization (must be able to view the project)
- [x] Link from the proofing UI and project detail
- [x] Snapshot tests for render output

**Deliverable:** P2 proofers can open "what did P1 change vs OCR?"

**Acceptance**
- Insertions/deletions are correct on samples with Vietnamese diacritics
- HTML injection in page text is escaped
- 3k-character diff < 20 ms

---

### Phase 8 — Browsing, comments, limits (2 weeks)

**Goal:** the site is navigable and social.

**Tasks**
- [ ] Project list with filters (round, availability, sort)
- [ ] Pagination (HTMX)
- [ ] Project detail: metadata, progress, event log
- [ ] Comments: post, render (`pulldown-cmark` + `ammonia`), HTMX append
- [ ] "My projects" for PMs
- [ ] Daily limit UI + clear blocked-state messaging
- [ ] Empty states and Vietnamese copy pass

**Deliverable:** a proofer can find work without a link from a developer.

**Acceptance**
- Filters combine correctly and are shareable via URL
- Comment HTML is sanitized (script tag stripped)
- Pagination has no off-by-one and caps page size at 100

---

### Phase 9 — Format preview (2 weeks)

**Goal:** proofers see the formatted result before saving.

**Tasks**
- [ ] Lexer for the markup subset (Appendix B)
- [ ] Parser producing a small AST
- [ ] HTML renderer with Spanish/French-style nesting rules
- [ ] Poetry and blockquote handling
- [ ] `<sc>` small caps, footnote markers
- [ ] Preview pane (HTMX) in the proofing UI
- [ ] Fuzz the lexer; snapshot the renderer

**Deliverable:** a preview pane that matches what a post-processor will expect.

**Acceptance**
- Unclosed markers render literally, never swallow the rest of the page
- Deep nesting does not blow the stack (depth cap + test)
- 3k characters < 5 ms

---

### Phase 10 — Admin (1.5 weeks)

**Goal:** the site can be run without SQL.

**Tasks**
- [ ] User list, role grant/revoke, disable
- [ ] Project oversight list, force transition, toggle `on_hold`
- [ ] Site word list management
- [ ] Settings page (registration open/closed, default limits)
- [ ] Audit log view

**Deliverable:** an admin can onboard a manager and unblock a stuck project.

**Acceptance**
- Role changes take effect on the next request
- Admin cannot demote the last admin
- Every destructive action is confirmed and audited

---

### Phase 11 — Operations and hardening (2 weeks)

**Goal:** deployable, recoverable, measureable.

**Tasks**
- [ ] Production Dockerfile (distroless), healthchecks, non-root user
- [ ] nginx config: TLS, compression, `X-Accel-Redirect`, cache headers, CSP
- [ ] Migration-on-deploy runbook, zero-downtime sequence
- [ ] Backup scripts + **restore drill performed and documented**
- [ ] Log rotation, `RUST_LOG` guidance
- [ ] Load test: 200 concurrent users, 20 saves/sec
- [ ] a11y audit of the proofing flow
- [ ] Error pages in Vietnamese
- [ ] Operations runbook: stuck claims, failed import, disk full, DB slow

**Deliverable:** production launch checklist signed off.

**Acceptance**
- Restore from backup into a clean environment reproduces the site
- Load test passes with no 5xx and no deadlocks
- CSP has no `unsafe-inline` for scripts
- Runbook covers the five most likely incidents

---

## 20. Milestones

| Milestone | After phase | Demonstrable |
|---|---|---|
| **M1 — Identity** | 2 | Register, verify, log in |
| **M2 — Import** | 3 | A book exists as a draft with 300 pages |
| **M3 — Movement** | 4 | Scripted project advances P1 → P2 |
| **M4 — Proofing** | 5 | A human proofs pages in a browser |
| **M5 — Quality** | 7 | Wordcheck and diffs are useful |
| **M6 — Product** | 9 | Browsable, social, formatted preview |
| **M7 — Launch** | 11 | Backups restore, load test passes |

M4 is the first demo worth showing to real proofers. Schedule a pilot with 3–5
users there; their feedback reshapes Phases 6–9 more cheaply than any internal
review.

---

## 21. Deployment and operations

### 21.1 Docker Compose (development and small production)

```yaml
services:
  db:
    image: mysql:8.4
    command: --character-set-server=utf8mb4 --collation-server=utf8mb4_vietnamese_ci
    environment:
      MYSQL_DATABASE: dp
      MYSQL_USER: dp
      MYSQL_PASSWORD: ${DB_PASSWORD}
      MYSQL_ROOT_PASSWORD: ${DB_ROOT_PASSWORD}
    volumes: [db-data:/var/lib/mysql]
    healthcheck:
      test: ["CMD", "mysqladmin", "ping", "-h", "127.0.0.1"]
      interval: 5s
      retries: 20

  app:
    build: .
    depends_on:
      db: { condition: service_healthy }
    environment:
      DP_DATABASE_URL: mysql://dp:${DB_PASSWORD}@db:3306/dp
    volumes:
      - dp-data:/var/lib/dp
      - ./config.toml:/etc/dp/config.toml:ro
    command: ["/usr/local/bin/dp", "serve"]
    restart: unless-stopped

  web:
    image: nginx:1.27-alpine
    depends_on: [app]
    ports: ["80:80", "443:443"]
    volumes:
      - ./deploy/nginx.conf:/etc/nginx/nginx.conf:ro
      - dp-data:/var/lib/dp:ro
      - ./static:/srv/static:ro
      - ./certs:/etc/nginx/certs:ro
    restart: unless-stopped

volumes:
  db-data:
  dp-data:
```

`static/` is built in CI and baked into the image in production; mounted in dev.

### 21.2 nginx essentials

```nginx
server {
    listen 443 ssl http2;
    server_name proof.example.org;

    client_max_body_size 512m;

    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options nosniff always;
    add_header Referrer-Policy same-origin always;
    add_header Content-Security-Policy "default-src 'self'; img-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; frame-ancestors 'none'" always;

    location /static/ {
        alias /srv/static/;
        expires 1y;
        add_header Cache-Control "public, immutable";
    }

    location /protected/ {
        internal;
        alias /var/lib/dp/;
        add_header Cache-Control "private, max-age=86400";
    }

    location / {
        proxy_pass http://app:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 60s;
    }
}
```

### 21.3 Migration runbook

```
1. Take a DB snapshot.
2. Deploy the new binary WITHOUT starting it.
3. Run `dp migrate`.
4. Start the new binary.
5. `curl -f /readyz` until 200.
6. Watch error rate for 15 minutes.
7. If it fails: stop, restore the snapshot, redeploy the previous binary.
```

Rules: migrations are additive within a release; destructive changes land one
release after the code stops using the old shape.

### 21.4 Monitoring minimum

| Signal | Alert |
|---|---|
| 5xx rate | > 1% for 5 min |
| p95 save latency | > 500 ms for 10 min |
| DB pool saturation | > 90% for 5 min |
| Disk on `data/` | > 80% |
| Stale-claim job | did not run in 30 min |
| `readyz` failing | 2 consecutive checks |

Logs: JSON to stdout, collected by the platform. Never to files inside the
container.

---

## 22. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | Round-advance race corrupts state | Medium | High | §6.3 locking; §6.7 tests are a release gate |
| R2 | Double claim of a page | Medium | High | Conditional `UPDATE` + `rows_affected`; no read-then-write |
| R3 | Wordcheck too noisy → proofers ignore it | High | Medium | Corpus calibration; < 0.5% FP target; pilot feedback at M4 |
| R4 | Wordcheck misses real errors | Medium | Medium | Bias toward flagging; project word lists; add signals iteratively |
| R5 | CM6 integration complexity (decorations, offsets) | Medium | Medium | Byte→UTF-16 conversion tested separately; fall back to plain textarea if blocked |
| R6 | Vietnamese phonotactic tables wrong | Medium | Medium | Native-speaker review; corpus calibration; advisory-only so errors are recoverable |
| R7 | IME bugs make typing painful | Medium | High | `compositionend` gating; explicit test on Telex input; pilot validation |
| R8 | Image storage growth | Medium | Low | Derivatives, cold archival, disk alerts |
| R9 | Scope creep (PP, SR, quizzes) | High | High | §24 list is explicit; new asks go to v2 backlog |
| R10 | Format preview drifts from post-processor expectations | Medium | Medium | Define the markup once in Appendix B; pilot with a real post-processor |
| R11 | Solo-dev bus factor | High | High | Phases are independently demonstrable; runbook + tests are the handover |
| R12 | MySQL `ONLY_FULL_GROUP_BY` surprises in reports | Low | Low | Write compliant SQL from day one; no legacy queries to port |

### The three that actually matter

1. **R1/R2** — concurrency. Everything else is recoverable; corrupted round
   state is not. Do not start Phase 5 until the concurrency suite is green.
2. **R3/R7** — if proofers find wordcheck annoying or typing laggy, they leave.
   Pilot at M4.
3. **R9** — this plan's value is its exclusions. Guard the out-of-scope list.

---

## 23. Open product decisions

These do not block Phase 1 but must be settled by the phase noted.

| # | Question | Needed by | Default if unresolved |
|---|---|---|---|
| Q1 | Exact format-preview markup subset (Appendix B is a proposal) | Phase 9 | Adopt Appendix B |
| Q2 | Is registration open, invite-only, or admin-approved? | Phase 2 | Open + email verification |
| Q3 | Accent-insensitive project search? | Phase 8 | No; exact with `utf8mb4_vietnamese_ci` |
| Q4 | Autosave drafts (vs explicit save only)? | Phase 5 | Explicit save only |
| Q5 | Who can mark a project `posted` — PM only, or PP? | Phase 10 | PM and PP |
| Q6 | Should `project_events` be visible to proofers? | Phase 8 | Managers and admins only |
| Q7 | Minimum pages before a user can proof P2 (20? 50?) | Phase 4 | 20 |

---

## 24. Deferred to v2

Deliberately excluded. Any of these arriving mid-project is a scope change and
needs a new plan, not an insertion.

- Post-processing tooling and PP-specific views
- Smooth reading rounds
- Quizzes / access qualification tests
- Task center, teams, mentors
- Statistics dashboards and charts
- Email notifications and digests
- Export formats (plain text, HTML, EPUB)
- Full-text search across page text
- Accent-insensitive search
- Multi-language UI
- Public API
- Forum or external discussion integration
- Mobile-native apps
- Horizontal scaling / multiple app instances (in-process rate limits assume one)

---

## 25. Appendices

### Appendix A — `rounds.toml`

```toml
# Compiled into the binary. Order defines progression.
# `min_pages` gates access based on total pages proofed in prior rounds.

[[round]]
id = "P1"
kind = "proof"
charset = "vi"
min_pages = 0

[[round]]
id = "P2"
kind = "proof"
charset = "vi"
min_pages = 20

[[round]]
id = "P3"
kind = "proof"
charset = "vi"
min_pages = 100

[[round]]
id = "F1"
kind = "format"
charset = "vi"
min_pages = 200

[[round]]
id = "F2"
kind = "format"
charset = "vi"
min_pages = 400
```

Validation at startup:

- IDs unique, non-empty, ≤ 8 chars
- Exactly one round with `min_pages = 0`
- `min_pages` non-decreasing
- Every `charset` exists in `charsets.toml`
- If no rounds: refuse to start

### Appendix B — Format preview markup (proposal)

A deliberately small subset. Anything not listed renders literally.

| Markup | Output |
|---|---|
| `_text_` | `<i>text</i>` |
| `*text*` | `<b>text</b>` |
| `=_text_=` | `<b><i>text</i></b>` |
| `<sc>text</sc>` | `<span class="sc">TEXT</span>` |
| `# text` at line start | `<blockquote><p>text</p></blockquote>` |
| `/>` at line start | poetry line (preserved indentation) |
| `[Footnote 1: text]` | footnote reference + list at end |
| `----` on its own line | `<hr>` |
| Blank line | paragraph break |

Rules:

- Unclosed markers render literally.
- Nesting depth capped at 8 (prevents pathological input).
- `_` inside a word does not open emphasis (`snake_case` survives).
- Vietnamese letters and diacritics are never markup.

This is a **proposal**. Phase 9 begins by confirming it with a real
post-processor and locking it in `dp-core/src/format_preview/spec.md`.

### Appendix C — HTTP status mapping

| Status | When |
|---|---|
| 200 | Rendered page or fragment |
| 201 | Resource created (HTMX only, rarely) |
| 303 | Successful POST → redirect (PRG) |
| 400 | Malformed input, character validation failure |
| 401 | No or expired session |
| 403 | Authenticated but not permitted; CSRF failure |
| 404 | Unknown project/page/user |
| 409 | Conflict: page claimed, page already saved, invalid transition |
| 413 | Upload too large |
| 422 | Semantically invalid form (shown inline) |
| 429 | Rate limited or daily limit reached |
| 500 | Unexpected; logged with a source chain |
| 503 | DB unavailable, pool exhausted |

### Appendix D — Glossary

| Term | Meaning |
|---|---|
| **Project** | One book, containing many pages |
| **Page** | One scanned image plus its OCR text |
| **Round** | A pass over the book: P1–P3 (proofing), F1–F2 (formatting) |
| **Pool** | The set of available pages in a round for a project |
| **Claim** | A proofer taking exclusive, time-limited ownership of a page |
| **Tally** | Count of pages a user has completed, per round per day |
| **Tone style** | Kiểu cũ (`hòa`) vs kiểu mới (`hoà`) placement of the tone mark |
| **Syllable** | The space-delimited unit of Vietnamese text; the unit of wordcheck |
| **Phonotactic** | Rule-based validity of a syllable's onset/nucleus/coda/tone |
| **NFC** | Unicode Normalization Form C (precomposed) — the canonical storage form |
| **CharSet** | The set of characters a project permits |
| **X-Accel-Redirect** | nginx mechanism: Rust authorizes, nginx serves the bytes |

---

## What to build first

If everything above is too much to hold at once, the order of operations that
matters is:

1. **Phase 1 + 2** — get a schema and an identity system that compiles and
   deploys. Nothing else is meaningful without it.
2. **Phase 4's concurrency design before Phase 5's UI.** Write the locking SQL
   and its tests while the schema is still soft. Retrofitting `FOR UPDATE` into
   a working proofing interface is the most expensive mistake available.
3. **Phase 5 before Phase 6.** Proofers need somewhere to type before wordcheck
   has anywhere to show up.
4. **Pilot at M4.** Three to five real proofers on a real 30-page book will
   reveal more than any of the remaining phases.

The plan is intentionally boring in the middle and paranoid at the edges. That
is where versions of this project usually fail.
