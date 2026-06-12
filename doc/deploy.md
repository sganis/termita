# Deploying termita

termita is a single self-contained binary with the Svelte frontend embedded
(`rust-embed`) — no Node, no `node_modules`, and no `ssh` client, nothing to install
at runtime. There are two ways to ship it:

- **A container image** — the root [`Dockerfile`](../Dockerfile) builds a static
  `x86_64-unknown-linux-musl` binary on `FROM scratch` (~3.4 MB). Run it with
  Docker/Podman or push it to any registry (§1, §3).
- **A scripted OpenShift rollout** — [`deploy/deploy.sh`](../deploy/deploy.sh) takes a
  **prebuilt Linux binary** and has OpenShift wrap it in a minimal UBI9 image with an
  in-cluster *COPY-only* build (no JS/Rust toolchain on the cluster), then rolls it
  out (§2). This is the easiest path onto a cluster like the Red Hat Developer Sandbox.

---

## Why this is easy on OpenShift

The previous (Node) image hit two OpenShift walls; both are gone:

- **Random UID.** OpenShift runs containers as an arbitrary UID in group 0. The old
  `ssh` client called `getpwuid()` and crashed with *"No user exists for uid …"*.
  The Rust binary never does that, so it runs under **any** UID — no `anyuid` SCC,
  no `/etc/passwd` hack. It works under the default `restricted-v2` SCC.
- **No in-cluster toolchain.** The OpenShift path (§2) ships a *prebuilt* binary; the
  only thing the cluster builds is a one-line `COPY` into a UBI9 base image. There is
  no bun/cargo download and no compile on the cluster.

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

## 2. Deploy to OpenShift with `deploy/deploy.sh` (recommended)

[`deploy/deploy.sh`](../deploy/deploy.sh) does the whole rollout: it applies the
manifests, uploads a prebuilt binary as the build context, lets OpenShift build a
minimal UBI9 image around it ([`deploy/Dockerfile`](../deploy/Dockerfile) — just a
`COPY`), and the Deployment's image trigger rolls the new image out. It prints the
public URL at the end.

### Prerequisites

- `oc`, logged into the target project. On the Sandbox, copy the login command from
  the console:
  ```bash
  oc login --token=sha256~… --server=https://api.<sandbox>.openshiftapps.com:6443
  ```
  It deploys into your **current** project (the Sandbox gives you `<user>-dev`); it
  does not create projects.
- `gh` (authenticated) — only if you let the script download the binary from CI. Not
  needed if you pass a local directory.

### Run it

```bash
# Use the prebuilt binary committed in the repo:
deploy/deploy.sh deploy/

# …or omit the dir to download the latest successful CI build
# (the `build` workflow's `termita-cloud-ubi9` artifact) via gh:
deploy/deploy.sh
```

`BUNDLE_DIR` is any directory containing a file named `termita`; the script finds it,
stages it next to `deploy/Dockerfile`, and uploads that as the build context.

### What it does

1. `oc apply -f deploy/openshift.yaml` — creates/updates an **ImageStream**, a
   **BuildConfig** (binary source, Docker strategy), the **Deployment** (with an image
   trigger), a **Service**, and an edge-TLS **Route**.
2. `oc start-build termita --from-dir=… --follow` — uploads the binary + Dockerfile;
   OpenShift builds `termita:latest` in-cluster by `COPY`ing the binary into
   `ubi9/ubi-minimal` (no compile).
3. The Deployment's image trigger rewrites the container image to the freshly built
   ImageStream pullspec and rolls it out. The script waits for `oc rollout status` and
   prints `https://<route-host>`.

The Deployment is aligned with the default **`restricted-v2`** SCC — no `runAsUser`,
plus `runAsNonRoot`, `readOnlyRootFilesystem`, all capabilities dropped — and sets
small CPU/memory requests with liveness/readiness probes on `/`. Restrict which SSH
targets are reachable by uncommenting **`ALLOWED_HOSTS`** in the Deployment's `env`
(see the manifest).

### Manual equivalent

```bash
oc apply -f deploy/openshift.yaml
oc start-build termita --from-dir=deploy/ --follow   # deploy/ holds termita + Dockerfile
oc rollout status deployment/termita
oc get route termita -o jsonpath='https://{.spec.host}{"\n"}'
```

> `deploy/` already contains both the committed `termita` binary and `Dockerfile`, so
> it works directly as the build context.

---

## 3. Alternative: push a prebuilt image to a registry

If you'd rather build the **container image** off-cluster and push it — to any registry
the cluster can pull from (Quay, GHCR, Docker Hub, or OpenShift's internal registry):

```bash
docker build -t quay.io/<you>/termita:0.1.0 .
docker push     quay.io/<you>/termita:0.1.0

oc new-app quay.io/<you>/termita:0.1.0 --name termita
oc create route edge termita --service=termita --insecure-policy=Redirect
oc get route termita -o jsonpath='https://{.spec.host}{"\n"}'
```

(The `deploy/openshift.yaml` manifest is wired for the in-cluster build of §2, not for
an externally pushed image — use `oc new-app` as above for that.)

For OpenShift's **internal registry** (an admin exposes it once with
`oc patch configs.imageregistry.operator.openshift.io/cluster --type=merge -p
'{"spec":{"defaultRoute":true}}'`):

```bash
REG=$(oc get route default-route -n openshift-image-registry -o jsonpath='{.spec.host}')
oc registry login --registry "$REG"
docker build -t "$REG/<project>/termita:0.1.0" .
docker push     "$REG/<project>/termita:0.1.0"
```

---

## 4. Restricted networks

The §2 flow needs **no internet on the cluster** beyond pulling the `ubi9/ubi-minimal`
base image (which OpenShift can normally pull from Red Hat's registry by default) — the
binary is built off-cluster (CI or your machine) and the in-cluster step is a single
`COPY`. There is no bun/cargo download in the cluster.

- If even the base image must come from an internal mirror, point the `FROM` in
  [`deploy/Dockerfile`](../deploy/Dockerfile) at it.
- The off-cluster binary build (CI's `build` workflow, or `cargo build --release`)
  needs internet for bun + cargo as usual; run it where you have it.

---

## 5. Security checklist for a real deployment

- **TLS:** the manifest's Route uses **edge** termination (HTTPS/WSS at the router with
  HTTP→HTTPS redirect), which encrypts the password in transit into the cluster.
- **`ALLOWED_HOSTS`:** set it to the hosts you actually need to reach — termita can
  otherwise SSH to anything the pod's network can reach.
- **Network policy:** restrict the pod's egress to the intended SSH subnets.
- **No credential storage:** the password authenticates the SSH session and is then
  dropped — never written to disk, logs, or args; the browser stores only
  host/username/port, never the password.
- **Host keys** are trust-on-first-use (not pinned); see `doc/spec.md`.

> Edge termination encrypts browser → router. Traffic router → pod is plaintext inside
> the cluster. If you need encryption all the way to the pod, use a
> `re-encrypt`/`passthrough` route and terminate TLS in a sidecar — termita itself does
> not serve TLS.

---

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `exec "/termita": permission denied` | The artifact lost its execute bit (zipped CI artifacts arrive `0644`). `deploy/Dockerfile` already `COPY --chmod=0755`s it — keep that if you edit it. |
| `no 'termita' binary found under <dir>` | The `BUNDLE_DIR` you passed has no file named `termita`. Point it at a dir that does (e.g. `deploy/`), or omit it to download from CI. |
| Build succeeds but the Deployment doesn't update | The rollout is driven by the image trigger on `termita:latest`. Check `oc get builds`, `oc describe deploy/termita`; the first apply + build can take a moment to roll out. |
| `CrashLoopBackOff`, no useful logs | `oc logs deploy/termita`; it prints `termita web-ssh on http://0.0.0.0:3000` on success. |
| Browser connects but SSH says "Could not resolve hostname" | The **target** host is unresolvable/blocked from the pod, or excluded by `ALLOWED_HOSTS`. Check pod egress + DNS. |
| WebSocket won't connect over the Route | Use `https://` (edge route); mixed `http`/`wss` is blocked by browsers. |
| Used to see *"No user exists for uid …"* | That was the old Node image; the Rust image runs under any UID. Make sure you redeployed the new image. |
