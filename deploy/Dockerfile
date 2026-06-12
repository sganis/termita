# deploy/Dockerfile
# In-cluster binary build for OpenShift: wrap the prebuilt glibc `termita` binary
# (from the GitHub Actions "termita-cloud-ubi9" bundle) in a minimal UBI9 image.
# No compilation happens here — just COPY the binary the bundle already contains, so
# the cluster builds nothing and ships no JS/Rust dependencies.
#
# The build context is the bundle dir, uploaded via `oc start-build --from-dir`.
FROM registry.access.redhat.com/ubi9/ubi-minimal:latest
# --chmod=0755: GitHub Actions artifacts are zipped and drop the Unix execute bit,
# so the downloaded binary arrives as 0644. Force it executable at COPY time, else
# the container fails with: exec "/termita": permission denied.
COPY --chmod=0755 termita /termita
ENV PORT=3000 \
    HOST=0.0.0.0
EXPOSE 3000
# Arbitrary non-root UID; OpenShift's restricted-v2 SCC overrides it anyway and
# russh never calls getpwuid, so any UID is fine.
USER 1001
ENTRYPOINT ["/termita"]
