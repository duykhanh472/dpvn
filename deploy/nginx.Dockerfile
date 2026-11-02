# Production nginx image with baked static assets.
#
# Built by docker-compose.prod.yml:
#   docker build -f deploy/nginx.Dockerfile -t dproofreaders-nginx .

FROM node:24-alpine AS assets
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY assets/ ./assets/
COPY scripts/ ./scripts/
RUN npm run build

FROM nginx:1.27-alpine

RUN rm -f /etc/nginx/conf.d/default.conf
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY deploy/security-headers.inc /etc/nginx/security-headers.inc
COPY --from=assets /app/static/ /srv/static/

# The stock entrypoint starts the master process as root so it can bind 80/443,
# then drops worker privileges to the `nginx` user. Do not set `USER nginx` here:
# an unprivileged master cannot bind the privileged ports.

HEALTHCHECK --interval=15s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -qO- http://127.0.0.1/healthz || exit 1

EXPOSE 80 443
