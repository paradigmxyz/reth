#!/usr/bin/env bash
set -euo pipefail

PROFILER_API="https://api.profiler.firefox.com"
PROFILER_ACCEPT="Accept: application/vnd.firefox-profiler+json;version=1.0"

for run_dir in baseline-1 baseline-2 feature-1 feature-2; do
  PROFILE="$BENCH_WORK_DIR/$run_dir/samply-profile.json.gz"
  if [ ! -f "$PROFILE" ]; then continue; fi

  PROFILE_SIZE=$(du -h "$PROFILE" | cut -f1)
  echo "Uploading $run_dir samply profile (${PROFILE_SIZE}) to Firefox Profiler..."

  JWT=$(curl -sf -X POST \
    -H "Content-Type: application/octet-stream" \
    -H "$PROFILER_ACCEPT" \
    --data-binary "@$PROFILE" \
    "$PROFILER_API/compressed-store") || {
    echo "::warning::Failed to upload $run_dir profile to Firefox Profiler"
    continue
  }

  PAYLOAD=$(echo "$JWT" | cut -d. -f2)
  case $(( ${#PAYLOAD} % 4 )) in
    2) PAYLOAD="${PAYLOAD}==" ;;
    3) PAYLOAD="${PAYLOAD}=" ;;
  esac
  PROFILE_TOKEN=$(echo "$PAYLOAD" | base64 -d 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['profileToken'])")
  PROFILE_URL="https://profiler.firefox.com/public/${PROFILE_TOKEN}"
  echo "Profile uploaded: $PROFILE_URL"

  SHORT_URL=$(curl -sf -X POST \
    -H "Content-Type: application/json" \
    -H "$PROFILER_ACCEPT" \
    -d "{\"longUrl\":\"$PROFILE_URL\"}" \
    "$PROFILER_API/shorten" | python3 -c "import sys,json; print(json.load(sys.stdin)['shortUrl'])" 2>/dev/null) || SHORT_URL="$PROFILE_URL"
  echo "$SHORT_URL" > "$BENCH_WORK_DIR/$run_dir/samply-profile-url.txt"
  echo "Short profile URL for $run_dir: $SHORT_URL"
done
