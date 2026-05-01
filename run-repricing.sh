#!/usr/bin/env bash
set -euo pipefail

if [ $# -ne 1 ]; then
  echo "Usage: $0 <name>" >&2
  exit 1
fi

ARG="$1"
BASE="repricings_stateful/perf-devnet-3"
JWT_HEX_PATH="/mnt/snapshot/datadir/jwt.hex"
URL="http://localhost:8551"

FILES=(
  "$BASE/gas-bump.txt"
  "$BASE/funding.txt"
  "$BASE/setup/$ARG"
  "$BASE/testing/$ARG"
)

for f in "${FILES[@]}"; do
  if [ ! -f "$f" ]; then
    echo "Missing file: $f" >&2
    exit 1
  fi
done

gen_jwt() {
  local secret
  secret=$(cat "$JWT_HEX_PATH")
  python3 -c "
import jwt, time
secret = bytes.fromhex('${secret}'.removeprefix('0x'))
print(jwt.encode({'iat': int(time.time())}, secret, algorithm='HS256'))
"
}

JWT=$(gen_jwt)
JWT_TS=$(date +%s)

for f in "${FILES[@]}"; do
  echo "==> $f"
  while IFS= read -r line; do
    [ -z "$line" ] && continue

    # Refresh JWT if older than 50s (iat tolerance is 60s)
    now=$(date +%s)
    if [ $((now - JWT_TS)) -ge 50 ]; then
      JWT=$(gen_jwt)
      JWT_TS=$now
    fi

    curl -sS -X POST "$URL" \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer $JWT" \
      -d "$line"
    echo
  done < "$f"
done
