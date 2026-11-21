#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

go tool oasmith --openapi contracts/public.yaml --mode client --lang go --out crates/sync-engine/publicapi
go tool oasmith --openapi contracts/client.yaml --mode types --lang go --strict --out crates/sync-engine/clientconfig
