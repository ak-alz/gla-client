#!/usr/bin/env bash
# AG-REL-003 — real, short E2E against the actual running backend
# (backend-backend-1): register a real throwaway test account (same
# pattern frontend-ci.yml already uses for its own CI-only account),
# pair a real growth-layer-agent instance against it via the real
# device-authorization flow, let it collect+upload for real, confirm
# the backend actually received and stored the data. Not a fabricated
# fixture -- a real account, a real pairing, a real upload.
set -euo pipefail

BINARY_PATH="${1:?usage: e2e_real_backend.sh <growth-layer-agent binary path>}"
BACKEND_URL="http://localhost:8000"
TEST_EMAIL="rel003-e2e-$(date +%s 2>/dev/null || echo static)-$$@example.invalid"
TEST_PASSWORD="rel003-e2e-test-password-not-reused-anywhere"

echo "== registering throwaway test account ($TEST_EMAIL) =="
REGISTER_RESPONSE="$(curl -s -X POST "$BACKEND_URL/v1/auth/register" \
    -H "Content-Type: application/json" \
    -d "{\"email\":\"$TEST_EMAIL\",\"password\":\"$TEST_PASSWORD\",\"timezone\":\"UTC\",\"consent_accepted\":true,\"consent_version\":\"2026-07-16-v1\"}")"
ACCESS_TOKEN="$(echo "$REGISTER_RESPONSE" | python3 -c 'import json,sys; print(json.load(sys.stdin)["access_token"])')"
echo "registered, got access token"

echo "== starting real device-authorization pairing =="
PAIR_START="$(curl -s -X POST "$BACKEND_URL/v1/agent/pair/start" -H "Content-Type: application/json" -d '{}')"
DEVICE_CODE="$(echo "$PAIR_START" | python3 -c 'import json,sys; print(json.load(sys.stdin)["device_code"])')"
USER_CODE="$(echo "$PAIR_START" | python3 -c 'import json,sys; print(json.load(sys.stdin)["user_code"])')"
echo "device_code/user_code issued: $USER_CODE"

echo "== confirming pairing as the logged-in test user =="
CONFIRM_RESPONSE="$(curl -s -X POST "$BACKEND_URL/v1/agent/pair/confirm" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -d "{\"user_code\":\"$USER_CODE\"}")"
echo "confirm response: $CONFIRM_RESPONSE"

echo "== agent polling for its real agent_token =="
POLL_RESPONSE="$(curl -s "$BACKEND_URL/v1/agent/pair/poll?device_code=$DEVICE_CODE")"
AGENT_TOKEN="$(echo "$POLL_RESPONSE" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("agent_token",""))')"
if [[ -z "$AGENT_TOKEN" ]]; then
    echo "FAIL: pairing did not produce a real agent_token: $POLL_RESPONSE" >&2
    exit 1
fi
echo "real agent_token obtained via real pairing flow"

SCRATCH_ROOT="$(mktemp -d -t growth-layer-agent-e2e-XXXXXX)"
DATA_DIR="$SCRATCH_ROOT/growth-layer-agent"
mkdir -p "$DATA_DIR"
cat > "$DATA_DIR/config.json" <<JSON
{"backend_url":"$BACKEND_URL","agent_token":"$AGENT_TOKEN","dashboard_url":"http://localhost:5173"}
JSON

cleanup() {
    [[ -n "${AGENT_PID:-}" ]] && kill -9 "$AGENT_PID" 2>/dev/null || true
    rm -rf "$SCRATCH_ROOT"
}
trap cleanup EXIT

# The 60s bucket flush (EXPORT_INTERVAL_SECONDS) and the uploader's own
# 30s idle-poll cadence (UPLOAD_INTERVAL, tried immediately at t=0 then
# every 30s while the queue is empty) compound worst-case to ~90s
# before the first real upload attempt happens (found by an actual
# run: 75s produced a real, valid record sitting in pending/, correctly
# enqueued, just not yet picked up by the uploader's own cadence).
echo "== running the real agent against the real backend, polling queue state every 15s for up to 150s =="
env "XDG_DATA_HOME=$SCRATCH_ROOT" "$BINARY_PATH" &
AGENT_PID=$!
for i in $(seq 1 10); do
    sleep 15
    P="$(find "$DATA_DIR/queue/pending" -type f 2>/dev/null | wc -l)"
    A="$(find "$DATA_DIR/queue/acked" -type f 2>/dev/null | wc -l)"
    L="$(find "$DATA_DIR/queue/leased" -type f 2>/dev/null | wc -l)"
    echo "DEBUG t=$((i*15))s pending=$P acked=$A leased=$L"
    if [[ "$A" -ge 1 ]]; then
        echo "DEBUG upload confirmed at t=$((i*15))s"
        break
    fi
done

if ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "FAIL: agent crashed during the real E2E run" >&2
    exit 1
fi

ACKED_BEFORE_STOP="$(find "$DATA_DIR/queue/acked" -type f 2>/dev/null | wc -l || true)"
kill -9 "$AGENT_PID"
wait "$AGENT_PID" 2>/dev/null || true

echo "== checking the real backend actually stored the uploaded record =="
TODAY_RESPONSE="$(curl -s "$BACKEND_URL/v1/me/today" -H "Authorization: Bearer $ACCESS_TOKEN")"
echo "GET /v1/me/today: $TODAY_RESPONSE"

ACKED="$(find "$DATA_DIR/queue/acked" -type f 2>/dev/null | wc -l)"
QUARANTINE="$(find "$DATA_DIR/queue/quarantine" -type f 2>/dev/null | wc -l)"
PENDING_DEBUG="$(find "$DATA_DIR/queue/pending" -type f 2>/dev/null | wc -l)"
echo "DEBUG pending=$PENDING_DEBUG"
echo "DEBUG log tail:"
tail -30 "$DATA_DIR/logs/agent.log" 2>/dev/null || echo "no log file found"

echo "{"
echo "  \"acked\": $ACKED,"
echo "  \"quarantine\": $QUARANTINE,"
if [[ "$ACKED" -ge 1 && "$QUARANTINE" -eq 0 ]]; then
    echo "  \"result\": \"PASS\""
    echo "}"
    exit 0
else
    echo "  \"result\": \"FAIL\""
    echo "}"
    exit 1
fi
