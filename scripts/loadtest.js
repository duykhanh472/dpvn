// k6 load test for the Phase 11 acceptance criteria:
//   * 200 concurrent users browsing
//   * 20 page saves per second
//   * no 5xx responses and p95 save latency < 500 ms
//
// Install k6: https://grafana.com/docs/k6/latest/set-up/install-k6/
//
// Prepare a dedicated project with enough pages for the run (see
// docs/operations/load-test.md), then:
//
//   DP_BASE_URL=https://proof.example.org \
//   DP_USERNAME=loadtest DP_PASSWORD='...' DP_PROJECT_ID=42 \
//   k6 run scripts/loadtest.js
//
// The script logs in once in setup() and reuses the session cookie. Each save
// iteration claims the next available page through /projects/{id}/proof, reads
// the CSRF token from the editor page, and saves once.

import http from "k6/http";
import { check, sleep } from "k6";
import { Counter, Trend } from "k6/metrics";

const BASE = (__ENV.DP_BASE_URL || "http://localhost:8080").replace(/\/$/, "");
const USERNAME = __ENV.DP_USERNAME;
const PASSWORD = __ENV.DP_PASSWORD;
const PROJECT_ID = __ENV.DP_PROJECT_ID;

const save5xx = new Counter("save_5xx");
const saveLatency = new Trend("save_latency_ms", true);

export const options = {
    scenarios: {
        browse: {
            executor: "constant-vus",
            vus: Number(__ENV.DP_BROWSE_VUS || 180),
            duration: __ENV.DP_DURATION || "5m",
            exec: "browse",
        },
        saves: {
            executor: "constant-arrival-rate",
            rate: Number(__ENV.DP_SAVE_RATE || 20),
            timeUnit: "1s",
            duration: __ENV.DP_DURATION || "5m",
            preAllocatedVUs: Number(__ENV.DP_SAVE_VUS || 40),
            maxVUs: 200,
            exec: "save",
        },
    },
    thresholds: {
        checks: ["rate>0.99"],
        save_5xx: ["count==0"],
        "http_req_duration{scenario:saves}": ["p(95)<500"],
        "http_req_duration{scenario:browse}": ["p(95)<1000"],
    },
};

export function setup() {
    if (!USERNAME || !PASSWORD || !PROJECT_ID) {
        throw new Error("DP_USERNAME, DP_PASSWORD and DP_PROJECT_ID are required");
    }

    const login = http.post(
        `${BASE}/login`,
        { username: USERNAME, password: PASSWORD },
        { redirects: 0 }
    );
    if (login.status !== 303 && login.status !== 302 && login.status !== 200) {
        throw new Error(`login failed with status ${login.status}`);
    }

    const setCookie = login.headers["Set-Cookie"] || login.headers["set-cookie"] || "";
    const cookie = setCookie.split(";")[0];
    if (!cookie.startsWith("dp_session=")) {
        throw new Error("login did not return a dp_session cookie");
    }

    return { cookie };
}

export function browse(data) {
    const headers = { Cookie: data.cookie };
    const list = http.get(`${BASE}/projects`, { headers });
    check(list, { "project list is 200": (r) => r.status === 200 });

    const detail = http.get(`${BASE}/projects/${PROJECT_ID}`, { headers });
    check(detail, { "project detail is 200": (r) => r.status === 200 });

    sleep(1);
}

export function save(data) {
    const headers = { Cookie: data.cookie };

    // Claiming is idempotent: an existing active claim redirects to the editor.
    const proof = http.get(`${BASE}/projects/${PROJECT_ID}/proof`, {
        headers,
        redirects: 10,
    });
    const csrf = match(proof.body, /data-csrf-token="([^"]+)"/);
    const page = match(proof.body, /data-page-number="(\d+)"/);
    if (!csrf || !page) {
        check(proof, { "editor page rendered": () => false });
        return;
    }

    const payload = JSON.stringify({
        text: `Trang kiểm tra tải số ${page}. Tiếng Việt có dấu: Nguyễn Việt Nam.`,
        confirmed_hash: null,
        csrf_token: csrf,
    });

    const started = Date.now();
    const response = http.post(
        `${BASE}/projects/${PROJECT_ID}/pages/${page}/save`,
        payload,
        {
            headers: {
                ...headers,
                "Content-Type": "application/json",
                "X-CSRF-Token": csrf,
            },
            redirects: 0,
        }
    );
    saveLatency.add(Date.now() - started);

    if (response.status >= 500) {
        save5xx.add(1);
    }
    check(response, {
        "save did not return 5xx": (r) => r.status < 500,
        "save returned a handled status": (r) =>
            [200, 201, 400, 403, 409, 429].includes(r.status),
    });
}

function match(body, regex) {
    const result = regex.exec(body || "");
    return result ? result[1] : null;
}
