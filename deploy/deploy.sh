#!/usr/bin/env bash
# deploy/deploy.sh — deploy termita to OpenShift from the GitHub Actions bundle.
#
# Flow: push -> GitHub Actions builds the "termita-cloud-ubi9" bundle -> run this
# from a machine logged into the cluster (`oc`) with `gh` authed:
#   1. downloads the latest successful build artifact (or uses a dir you pass),
#   2. applies the OpenShift manifests (ImageStream/BuildConfig/Deployment/Service/Route),
#   3. `oc start-build --from-dir` uploads the bundle; OpenShift builds a COPY-only
#      UBI9 image in-cluster (no JS/Rust build) and the image trigger rolls it out.
#
# Usage:
#   deploy/deploy.sh [BUNDLE_DIR]
#     BUNDLE_DIR  optional dir containing the `termita` binary. If omitted, the
#                 latest successful `build` workflow artifact is downloaded via gh.
#
# Target cluster: whatever `oc` is logged into — e.g. the Red Hat Developer Sandbox.
# Log in first with the "Copy login command" token from the Sandbox console:
#   oc login --token=sha256~... --server=https://api.<sandbox>.openshiftapps.com:6443
# It deploys into your current project (the Sandbox gives you <user>-dev); it does
# not create projects.
#
# Requires: oc (logged into the target project) and, when downloading, gh (authed
# with access to this repo). Run it from the repo (gh infers the repo) or set
# GH_REPO=sganis/termita.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
app=termita
manifest="$here/openshift.yaml"
dockerfile="$here/Dockerfile"

oc whoami >/dev/null 2>&1 || { echo "Not logged in. Run the 'oc login ...' command from your Sandbox console first." >&2; exit 1; }
echo "==> deploying to project '$(oc project -q)' as '$(oc whoami)'"

bundle="${1:-}"
tmp=""
if [[ -z "$bundle" ]]; then
  command -v gh >/dev/null || { echo "gh CLI required to download the bundle (or pass a BUNDLE_DIR)." >&2; exit 1; }
  echo "==> resolving latest successful 'build' run"
  rid="$(gh run list --workflow build.yml --status success --limit 1 --json databaseId --jq '.[0].databaseId')"
  [[ -n "$rid" ]] || { echo "No successful 'build' run found — push to trigger the build workflow first." >&2; exit 1; }
  tmp="$(mktemp -d)"
  echo "==> downloading bundle from run $rid"
  gh run download "$rid" --name termita-cloud-ubi9 --dir "$tmp"
  bundle="$tmp"
fi

binary="$(find "$bundle" -type f -name "$app" | head -1)"
[[ -n "$binary" ]] || { echo "ERROR: no '$app' binary found under $bundle" >&2; exit 1; }

# Stage the build context: the binary + the COPY-only Dockerfile.
ctx="$(mktemp -d)"
trap 'rm -rf "$ctx" ${tmp:+"$tmp"}' EXIT
cp "$binary" "$ctx/$app"
cp "$dockerfile" "$ctx/Dockerfile"
chmod +x "$ctx/$app"

echo "==> applying manifests"
oc apply -f "$manifest"

echo "==> uploading bundle; OpenShift builds the UBI9 image in-cluster"
oc start-build "$app" --from-dir="$ctx" --follow

echo "==> waiting for rollout"
oc rollout status "deployment/$app" --timeout=180s

host="$(oc get route "$app" -o jsonpath='{.spec.host}' 2>/dev/null || true)"
[[ -n "$host" ]] && echo "==> termita is up: https://$host"
