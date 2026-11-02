# DProofreaders VN (Rust)

A Rust rewrite of [Distributed Proofreaders](https://www.pgdp.net) with
first-class support for Vietnamese text.

## Description

DProofreaders VN is a web application that turns scanned books into
public-domain e-texts. Volunteers ("proofers") claim individual pages, see the
page image beside the OCR text, correct it, run a Vietnamese-aware spell check,
preview the formatting markup, and submit their work for the next round. Project
managers create projects, upload page scans, and monitor progress; administrators
manage users, word lists, site settings, and review an audit log of every
destructive action.

The server is written in Rust (Axum + MiniJinja + SQLx) and stores everything in
MySQL. The frontend is server-rendered HTML enhanced with htmx, CodeMirror, and a
small amount of progressive-enhancement JavaScript — no client-side framework.

The workspace is split into three crates:

| Crate | Responsibility |
|---|---|
| `dp-core` | Pure domain logic: Vietnamese normalization, wordcheck, tone-style rules, charset palettes, round transitions, diffing, format preview |
| `dp-db` | SQLx models, queries, and embedded migrations |
| `dp-web` | HTTP routes, templates, middleware, ingest pipeline, CLI (`dp`) |

Feature areas implemented today: registration and sessions, project catalog with
filters, page proofing with claims and daily limits, wordcheck, character palette,
diff view, format preview, comments, admin console, structured logging, health
endpoints, and a hardened deployment path (distroless image, TLS nginx, backups,
restore drills).

## Getting Started

### Dependencies

- **Rust 1.85 or newer** (the workspace uses edition 2024). Install with
  [rustup](https://rustup.rs/).
- **MySQL 8.4** (the version used in CI and containers). The database and tables
  must use `utf8mb4` with `utf8mb4_vietnamese_ci` collation.
- **Node.js 20 or newer** and npm — only needed to bundle frontend assets.
  The Docker image uses Node 24.
- **Docker 24+ with the Compose plugin** (optional) for the one-command
  development stack.
- **k6** (optional) to run the load test, and **shellcheck** (optional) to lint
  the deploy scripts.
- Linux or macOS. Windows works through WSL2.

### Installing

1. Get the sources:

   ```sh
   git clone <repository-url> dproofreaders-rust
   cd dproofreaders-rust
   ```

2. Build the frontend assets. This writes hashed bundles and a manifest into
   `static/`:

   ```sh
   npm ci
   npm run build
   ```

3. Create a MySQL database and user:

   ```sql
   CREATE DATABASE dp CHARACTER SET utf8mb4 COLLATE utf8mb4_vietnamese_ci;
   CREATE USER 'dp'@'%' IDENTIFIED BY 'change-me';
   GRANT ALL PRIVILEGES ON dp.* TO 'dp'@'%';
   ```

   With Docker instead:

   ```sh
   docker run -d --name dp-db -p 127.0.0.1:3306:3306 \
     -e MYSQL_DATABASE=dp -e MYSQL_USER=dp -e MYSQL_PASSWORD=change-me \
     -e MYSQL_ROOT_PASSWORD=change-root \
     mysql:8.4 \
     --character-set-server=utf8mb4 --collation-server=utf8mb4_vietnamese_ci
   ```

4. Review `config.toml` and adjust the database URL, bind address, data
   directory, SMTP relay, and limits. Any setting can be overridden with an
   environment variable (`DP_DATABASE_URL`, `DP_BIND`, `DP_PUBLIC_URL`,
   `DP_DATA_DIR`, `DP_MAX_CONNECTIONS`, …), which is how the containers are
   configured. Never commit real credentials.

5. Build the server:

   ```sh
   cargo build --release
   ```

   The binary is `target/release/dp`.

6. Apply migrations and create the first administrator:

   ```sh
   export DP_DATABASE_URL='mysql://dp:change-me@127.0.0.1:3306/dp'
   ./target/release/dp migrate
   DP_ADMIN_PASSWORD='a-strong-password' ./target/release/dp create-admin admin
   ```

### Executing program

The quickest path is the development Compose stack, which starts MySQL, the app,
and an nginx frontend on <http://localhost:8080>:

```sh
cp .env.example .env      # edit secrets first
docker compose up --build
```

Run natively during development instead:

```sh
export DP_DATABASE_URL='mysql://dp:change-me@127.0.0.1:3306/dp'
export DP_DEV_MODE=1      # reload templates from disk
cargo run -p dp-web -- serve
```

Then open <http://127.0.0.1:8080>.

Other useful subcommands:

```sh
# Show every command and option.
./target/release/dp --help

# Import a scanned book package (zip of page images + OCR text).
./target/release/dp import-project book.zip --name "Tên sách" \
  --author "Tác giả" --tone-style either

# Run periodic maintenance: release stale claims, expire sessions,
# prune pending users, old temp files, and orphaned images.
./target/release/dp prune

# Check configuration, templates, database, and file consistency.
./target/release/dp verify

# Probe the running server (used by the container healthcheck).
./target/release/dp healthcheck --url http://127.0.0.1:8080/healthz
```

Production deployment uses `Dockerfile` and `docker-compose.prod.yml`. The
zero-downtime release sequence, TLS setup, backup/restore, and rollback steps are
documented in [`docs/operations/deploy.md`](docs/operations/deploy.md).

## Help

Run the built-in help for the authoritative command list:

```sh
./target/release/dp --help
```

Common problems:

- **`pool timed out while waiting for an open connection`** — MySQL is not
  reachable, not ready yet, or `DP_DATABASE_URL` is wrong. Verify with
  `mysqladmin ping`, check the container health, and confirm the database
  exists with the `utf8mb4_vietnamese_ci` collation.
- **Missing styles or `/static/...` returns 404** — frontend assets were not
  built, or `DP_MANIFEST_PATH` points at the wrong manifest. Run
  `npm ci && npm run build` and check that `static/manifest.json` exists.
- **Migrations fail on collation or charset** — recreate the schema with
  `CHARACTER SET utf8mb4 COLLATE utf8mb4_vietnamese_ci`; mixing collations in
  joins and unique indexes will fail.
- **`Permission denied` writing page images** — the process needs write access
  to the directory in `[paths].data` (or `DP_DATA_DIR`). In containers that is
  `/var/lib/dp`, owned by UID/GID `65532`.
- **Activation emails never arrive** — check `[mail].smtp_url` and that the SMTP
  relay accepts the configured `from` address.
- **Templates do not change after an edit** — production builds embed templates;
  set `DP_DEV_MODE=1` (or mount a templates directory with
  `DP_TEMPLATES_DIR`) to reload from disk.

More documentation:

- [`docs/operations/`](docs/operations/README.md) — deployment, backups,
  logging, load testing, accessibility, incident runbook, launch checklist.
- [`scripts/check-hardening.sh`](scripts/check-hardening.sh) — static checks for
  the container, nginx, CSP, and operational guarantees.
- [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md) — architecture and phase
  plan.

## Authors

Distributed Proofreaders Team — see the `authors` field in `Cargo.toml`.

Maintainers and contributors are listed by `git shortlog -sne`. This project is
a rewrite of the original PHP [Distributed Proofreaders](https://github.com/DistributedProofreaders/dproofreaders)
codebase; the volunteers behind that project and the proofreaders who use it
made this work possible.

## Version History

* 0.1.0
    * Initial Rust release covering phases 1–11: authentication and sessions,
      project catalog, page proofing with claims and daily limits, Vietnamese
      wordcheck and character palette, diff view, format preview, comments,
      admin console, and production operations hardening.
    * No tagged releases yet; see the commit history for details.

## License

This project is licensed under **MIT OR Apache-2.0**, at your option, as declared
in [`Cargo.toml`](Cargo.toml). Contributions are accepted under the same terms.

## Acknowledgments

* [Distributed Proofreaders](https://www.pgdp.net) and
  [Project Gutenberg](https://www.gutenberg.org) for the original platform,
  workflow, and public-domain corpus.
* The Rust ecosystem this project builds on: [Axum](https://github.com/tokio-rs/axum),
  [SQLx](https://github.com/launchbadge/sqlx), [MiniJinja](https://github.com/mitsuhiko/minijinja),
  [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark),
  [ammonia](https://github.com/rust-ammonia/ammonia), [argon2](https://github.com/RustCrypto/password-hashes),
  and [similar](https://github.com/mitsuhiko/similar).
* Frontend building blocks: [htmx](https://htmx.org), [CodeMirror](https://codemirror.net),
  and [esbuild](https://esbuild.github.io).
* README structure inspired by [PurpleBooth](https://gist.github.com/PurpleBooth/109311bb0361f32d87a2),
  [dbader](https://github.com/dbader/readme-template), and
  [awesome-readme](https://github.com/matiassingers/awesome-readme).
