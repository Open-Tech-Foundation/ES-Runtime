#!/usr/bin/env bash
# Stands up an S3-compatible server and prints the environment e2e.mjs wants.
#
#   eval "$(test/s3-server.sh)" && test/run.sh
#   test/s3-server.sh --plain >> "$GITHUB_ENV"   # KEY=value lines, for CI
#   docker rm -f esrun-s3   # when you are done
#
# RustFS (Apache-2.0), pinned. MinIO's repository is archived and says it is no
# longer maintained. The image needs no arguments, so it could run as a service
# container too.
set -euo pipefail

name="${S3_NAME:-esrun-s3}"
port="${S3_PORT:-9000}"
image="${S3_IMAGE:-rustfs/rustfs:1.0.4}"
access="esrunaccess"
secret="esrun-secret-$(date +%s)"

docker pull -q "$image" >/dev/null
docker rm -f "$name" >/dev/null 2>&1 || true
docker run -d --name "$name" -p "127.0.0.1:$port:9000" \
  -e RUSTFS_ACCESS_KEY="$access" -e RUSTFS_SECRET_KEY="$secret" \
  "$image" >/dev/null

for _ in $(seq 1 60); do
  # Any HTTP answer means it is listening; an unsigned request is a 403.
  if curl -s -o /dev/null "http://127.0.0.1:$port/"; then break; fi
  sleep 0.5
done

prefix="export "
[ "${1:-}" = "--plain" ] && prefix=""
echo "${prefix}S3_E2E_ENDPOINT=http://127.0.0.1:$port"
echo "${prefix}S3_E2E_ACCESS_KEY=$access"
echo "${prefix}S3_E2E_SECRET_KEY=$secret"
