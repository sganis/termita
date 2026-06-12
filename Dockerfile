# Dockerfile
# ---------- build stage: Bun installs deps, builds the Svelte 5 bundle ----------
FROM registry.access.redhat.com/ubi9/nodejs-20 AS build

USER 0

# unzip is needed by the Bun installer.
RUN dnf install -y unzip tar gzip && dnf clean all

# Install Bun -> /usr/local/bin/bun
ENV BUN_INSTALL=/usr/local
RUN curl -fsSL https://bun.sh/install | bash

WORKDIR /build

# Deps first for layer caching.
COPY package.json bun.lock ./
RUN bun install

# Build the Svelte 5 frontend -> web/dist, then prune to production deps
# (runtime only needs node-pty + ws; node-pty ships a prebuilt binary).
COPY . .
RUN bun run build && rm -rf node_modules && bun install --production

# ---------- runtime stage: minimal UBI9 Node + node-pty ----------
FROM registry.access.redhat.com/ubi9/nodejs-20-minimal

USER 0

# termita only relays to a remote shell — it needs the ssh client (and a shell
# for the entrypoint). The remote host provides vim/top/etc.
RUN microdnf install -y --nodocs \
    bash \
    openssh-clients \
    && microdnf clean all

# The minimal base has no passwd entry for uid 1001; the ssh client calls
# getpwuid() and fails ("No user exists for uid 1001") without one.
RUN echo 'termita:x:1001:0:termita:/tmp:/bin/bash' >> /etc/passwd

ENV HOME=/tmp \
    TERM=xterm-256color \
    NODE_ENV=production \
    PORT=3000 \
    HOST=0.0.0.0

WORKDIR /app

COPY --from=build /build/node_modules ./node_modules
COPY --from=build /build/web/dist ./web/dist
COPY --from=build /build/server.js ./

EXPOSE 3000
USER 1001

ENTRYPOINT ["node", "/app/server.js"]
