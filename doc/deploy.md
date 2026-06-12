# Deploying termita

termita ships as a single static `x86_64-unknown-linux-musl` binary on a `FROM
scratch` image (~3.4 MB) with the frontend embedded. There is no Node, no
`node_modules`, and no `ssh` client in the image — so deployment is just "run one
container".

This guide covers Docker/Podman and **OpenShift** (the original target). The short
version: **build the image where you have internet, push it to a registry, and run
it.** Don't make the cluster build it unless you have to.

---

## Why this is now easy on OpenShift

The previous (Node) image hit two OpenShift walls; both are gone:

- **Random UID.** OpenShift runs containers as an arbitrary UID in group 0. The old
  `ssh` client called `getpwuid()` and crashed with *"No user exists for uid …"*.
  The Rust binary never does that, so it runs under **any** UID — no `anyuid` SCC,
  no `/etc/passwd` hack. It works under the default `restricted-v2` SCC.
- **Build-time network.** Nothing is installed at runtime. The only network the
  *build* needs is for bun + cargo dependency downloads, which you do off-cluster.

---

## 1. Run locally (Docker / Podman)

```bash
docker build -t termita .
docker run --rm -it -p 127.0.0.1:3000:3000 termita
# open http://localhost:3000
```

Configuration is via environment variables:

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `3000` | listen port |
| `HOST` | `0.0.0.0` | bind address inside the container |
| `ALLOWED_HOSTS` | _(empty = any)_ | comma-separated allowlist of SSH targets |

```bash
docker run --rm -p 127.0.0.1:3000:3000 \
  -e ALLOWED_HOSTS=10.0.0.5,10.0.0.6 termita
```

> Bind to `127.0.0.1` and front it with TLS for anything beyond local use — the
> password travels browser → server over the WebSocket.

---

## 2. Deploy to OpenShift — build off-cluster, push, run (recommended)

### 2a. Build and push the image

Use any registry the cluster can pull from (Quay, Docker Hub, GHCR, or OpenShift's
internal registry).

**External registry (simplest):**

```bash
docker build -t quay.io/<you>/termita:0.1.0 .
docker push     quay.io/<you>/termita:0.1.0
```

**OpenShift internal registry** (needs the registry route exposed — admin does this
once: `oc patch configs.imageregistry.operator.openshift.io/cluster \
  --type=merge -p '{"spec":{"defaultRoute":true}}'`):

```bash
oc login ... && oc project myproject
REG=$(oc get route default-route -n openshift-image-registry -o jsonpath='{.spec.host}')
oc registry login --registry "$REG"
docker build -t "$REG/myproject/termita:0.1.0" .
docker push     "$REG/myproject/termita:0.1.0"
```

### 2b. Deploy

Edit the image reference in [`deploy/openshift.yaml`](../deploy/openshift.yaml), then:

```bash
oc apply -f deploy/openshift.yaml
oc get route termita -o jsonpath='https://{.spec.host}{"\n"}'   # the URL to open
```

That manifest creates a **Deployment + Service + Route**. The Route uses **edge TLS**
(`https`/`wss` at the router, with HTTP→HTTPS redirect), which is what encrypts the
password in transit into the cluster. WebSockets pass through OpenShift routes
without extra config.

> Edge termination encrypts browser → router. Traffic router → pod is plaintext
> inside the cluster. If you need encryption all the way to the pod, use a
> `re-encrypt`/`passthrough` route and terminate TLS in a sidecar — termita itself
> does not serve TLS.

### One-liner alternative (no manifest file)

```bash
oc new-app quay.io/<you>/termita:0.1.0 --name termita
oc create route edge termita --service=termita --insecure-policy=Redirect
oc get route termita -o jsonpath='https://{.spec.host}{"\n"}'
```

---

## 3. Let OpenShift build it (binary BuildConfig)

Only if you can't push from your machine. The build pod needs outbound network for
bun + cargo (see *Restricted networks* below if it doesn't have it).

```bash
oc new-build --name termita --binary --strategy docker
oc start-build termita --from-dir=. --follow      # streams the 3-stage build
oc new-app termita
oc create route edge termita --service=termita --insecure-policy=Redirect
```

---

## 4. Restricted networks (offline in-cluster builds)

If build pods have no internet, the bun + cargo downloads fail. Options:

- **Preferred:** build off-cluster (section 2) and push — the cluster only *pulls*.
- **Vendor cargo deps:** `cargo vendor vendor/` and commit it with a
  `.cargo/config.toml` pointing at the vendor dir, so `cargo build` is offline.
- **Frontend:** point bun at an internal npm mirror, or pre-build `web/dist` and
  `COPY` it instead of running `bun run build` in the image.
- Use internal mirrors for the `oven/bun` and `rust:1-alpine` base images.

---

## 5. Security checklist for a real deployment

- **TLS:** use the edge Route (section 2b) so the password is encrypted in transit.
- **`ALLOWED_HOSTS`:** set it to the hosts you actually need to reach — termita can
  otherwise SSH to anything the pod's network can reach.
- **Network policy:** restrict the pod's egress to the intended SSH subnets.
- **No credential storage:** the password authenticates the SSH session and is then
  dropped — never written to disk, logs, or args; the browser stores only
  host/username/port, never the password.
- **Host keys** are trust-on-first-use (not pinned); see `doc/spec.md`.

---

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Build fails downloading bun/cargo deps | Build pod has no internet — build off-cluster and push (section 2), or vendor deps (section 4). |
| `CrashLoopBackOff`, no useful logs | Check `oc logs deploy/termita`; it prints `termita web-ssh on http://0.0.0.0:3000` on success. |
| Browser connects but SSH says "Could not resolve hostname" | The **target** host is unresolvable/blocked from the pod, or excluded by `ALLOWED_HOSTS`. Check pod egress + DNS. |
| WebSocket won't connect over the Route | Confirm you're using `https://` (edge route); mixed `http`/`wss` is blocked by browsers. |
| Used to see *"No user exists for uid …"* | That was the old Node image; the Rust image runs under any UID. Make sure you redeployed the new image. |
