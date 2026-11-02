# Load test: 200 concurrent users, 20 saves/second

Phase 11 acceptance requires a run with **no 5xx responses and no deadlocks**
at 200 concurrent users and 20 saves per second, with p95 save latency under
500 ms.

## 1. What the script does

`scripts/loadtest.js` (k6) runs two scenarios in parallel:

| Scenario | Executor | Load | Requests |
|---|---|---|---|
| `browse` | constant-vus | 180 VUs for 5 min | `GET /projects`, `GET /projects/{id}` |
| `saves` | constant-arrival-rate | 20 iterations/s for 5 min | `GET /projects/{id}/proof` (claim) then `POST .../save` |

The save scenario is a real claim → edit → save loop: it follows the redirect
to the editor, reads the CSRF token and page number from the page, and posts a
valid Vietnamese text. It counts any 5xx in the `save_5xx` metric and fails the
run if one occurs.

## 2. Prepare the environment

Use a **dedicated** project and user. Do not point this at production data.

1. Create a load-test user and activate it (or use `dp create-admin` and grant
   the `proofer` role).
2. Import a project with enough pages for `rate × duration`:
   `20 × 300 = 6000` pages minimum, plus headroom for retries. 10,000 is safe.
3. Make sure the project is `in_round`, not on hold, and that the user has
   access to the round (the P1 round has `min_pages = 0`).
4. Raise the user's daily limit for the run, or set
   `settings.daily_pages` to a large value, otherwise saves start returning
   `429` after 100 pages. Restore the normal limit afterwards.
5. Size the app and database for the test: at least 4 app cores and a MySQL
   instance with `max_connections >= 200` and a buffer pool large enough for
   the working set.

## 3. Install and run k6

```sh
# Debian/Ubuntu
sudo apt-get install k6
# or: https://grafana.com/docs/k6/latest/set-up/install-k6/

DP_BASE_URL=https://staging.example.org \
DP_USERNAME=loadtest \
DP_PASSWORD='...' \
DP_PROJECT_ID=42 \
DP_DURATION=5m \
k6 run scripts/loadtest.js
```

Useful overrides:

| Variable | Default | Meaning |
|---|---|---|
| `DP_BROWSE_VUS` | 180 | concurrent browsing users |
| `DP_SAVE_RATE` | 20 | saves per second |
| `DP_SAVE_VUS` | 40 | pre-allocated VUs for the save scenario |
| `DP_DURATION` | `5m` | scenario duration |

## 4. Pass criteria

The run passes when k6 exits 0 and all thresholds hold:

```
checks.........................: 100.00% ✓ ...
save_5xx.......................: 0
http_req_duration{scenario:saves}...: p(95)<500ms
http_req_duration{scenario:browse}..: p(95)<1000ms
```

Additionally, check server-side:

```sh
# No 5xx in the access log for the test window.
docker compose -f docker-compose.prod.yml logs app \
  | grep '"status":5' | grep -v healthz

# No deadlocks or lock-wait timeouts in MySQL.
docker compose -f docker-compose.prod.yml exec db \
  mysql -uroot -p"$DB_ROOT_PASSWORD" -e "SHOW GLOBAL STATUS LIKE 'Innodb_deadlocks';"
```

Record the result below (and attach the k6 summary to the launch ticket).

## 5. Result record

| Date | Build | Environment | Browse VUs | Saves/s | p95 save | 5xx | Deadlocks | Result |
|---|---|---|---|---|---|---|---|---|
| _pending_ | _commit_ | _staging_ | 180 | 20 | _ms_ | 0 | 0 | _pending_ |

## 6. Cleanup

- Restore the daily page limit and disable/delete the load-test user.
- Archive or delete the load-test project and its images.
- If the run created thousands of `page_versions`, run `dp prune` and confirm
  the version-retention job keeps only the newest 20 per `(page, round)`.
