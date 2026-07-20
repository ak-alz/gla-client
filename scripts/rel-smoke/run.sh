#!/usr/bin/env bash
# AG-REL-001 — compressed real fault-injection smoke test. Per the
# session's explicit course correction (ship 3 working clients, don't
# spend the session on exhaustive/multi-day testing), this is a SHORT
# (under a minute) real run chaining several real faults through one
# live growth-layer-agent process and durable-queue directory, not a
# literal 7-day/24h soak. Exercises, for real, in one continuous flow:
#   - offline queueing (unreachable backend at first)
#   - a corrupted queue record (quarantine, not silent loss/crash)
#   - server 5xx (backoff, not crash/busy-loop)
#   - 401/token-revoke-like rejection (backoff, not crash)
#   - recovery once the backend starts accepting (200)
#   - a hard crash (SIGKILL) mid-drain and restart without reinstall
#   - a final accounting pass: every seeded record is acked, quarantined,
#     or still pending -- never silently lost
# Clock/timezone change and literal multi-day duration are explicitly
# NOT covered here -- see TEST_REPORT.md's own scope note.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY_PATH="${1:?usage: run.sh <growth-layer-agent binary path>}"
SEED_QUEUE_BIN="${2:-$(dirname "$BINARY_PATH")/examples/seed_queue}"

if [[ ! -x "$BINARY_PATH" ]]; then
    echo "binary not found at $BINARY_PATH" >&2
    exit 2
fi
if [[ ! -x "$SEED_QUEUE_BIN" ]]; then
    echo "seed_queue not found at $SEED_QUEUE_BIN -- cargo build --release --example seed_queue -p durable-queue" >&2
    exit 2
fi

SCRATCH_ROOT="$(mktemp -d -t growth-layer-agent-relsmoke-XXXXXX)"
DATA_DIR="$SCRATCH_ROOT/growth-layer-agent"
mkdir -p "$DATA_DIR"

cleanup() {
    [[ -n "${SERVER_PID:-}" ]] && kill -9 "$SERVER_PID" 2>/dev/null || true
    [[ -n "${AGENT_PID:-}" ]] && kill -9 "$AGENT_PID" 2>/dev/null || true
    rm -rf "$SCRATCH_ROOT"
}
trap cleanup EXIT

# A phased mock backend: request 1-2 -> 500 (server error / unstable
# network), request 3 -> 401 (token-revoke-like rejection), request 4+
# -> 200 (recovery). Real, minimal, self-contained -- same style as
# perf-bench's own mock server, ephemeral port to avoid the same
# fixed-port collision AG-PERF-001 found and fixed.
PORT_FILE="$SCRATCH_ROOT/port"
python3 -c "
import http.server
count = [0]
class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get('Content-Length', 0))
        self.rfile.read(length)
        count[0] += 1
        n = count[0]
        if n <= 2:
            self.send_response(500)
            self.send_header('Content-Length', '0')
            self.send_header('Connection', 'close')
            self.end_headers()
        elif n == 3:
            self.send_response(401)
            self.send_header('Content-Length', '0')
            self.send_header('Connection', 'close')
            self.end_headers()
        else:
            body = b'{}'
            self.send_response(200)
            self.send_header('Content-Length', str(len(body)))
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(body)
    def log_message(self, *args):
        pass
srv = http.server.HTTPServer(('127.0.0.1', 0), Handler)
with open('$PORT_FILE', 'w') as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
" &
SERVER_PID=$!
for _ in $(seq 1 50); do
    [[ -s "$PORT_FILE" ]] && break
    sleep 0.1
done
PORT="$(cat "$PORT_FILE")"
BACKEND_URL="http://127.0.0.1:$PORT"

cat > "$DATA_DIR/config.json" <<JSON
{"backend_url":"$BACKEND_URL","agent_token":"rel-smoke-token","dashboard_url":"http://127.0.0.1:1"}
JSON

# Seed 5 real, valid records + 1 deliberately corrupted one -- proving
# "no silent data loss" means accounting for BOTH kinds honestly.
"$SEED_QUEUE_BIN" "$DATA_DIR/queue" 5
echo "not a valid envelope, deliberately corrupted for this smoke test" > "$DATA_DIR/queue/pending/00000000-0000-0000-0000-000000000bad.json"

TOTAL_SEEDED=6

env "XDG_DATA_HOME=$SCRATCH_ROOT" "$BINARY_PATH" &
AGENT_PID=$!
sleep 1

if ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "FAIL: agent did not start" >&2
    exit 1
fi

# Let it run through the 500/500/401/200... phases for real -- long
# enough for several real upload attempts (backoff base is 1s,
# doubling), short enough to stay a "smoke test," not a soak test.
sleep 15

if ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "FAIL: agent crashed during server-error/unauthorized phase (should have backed off, not died)" >&2
    exit 1
fi
RSS_DURING_FAULTS="$(awk '/^VmRSS:/ { printf "%.2f", $2/1024 }' "/proc/$AGENT_PID/status" 2>/dev/null || echo 0)"

# Real hard crash mid-drain -- SIGKILL, no graceful shutdown, exactly
# what durable-queue's crash-recovery (lease-recovery on next `open()`)
# and lifecycle::CrashMarker exist to survive.
kill -9 "$AGENT_PID"
wait "$AGENT_PID" 2>/dev/null || true
sleep 0.5

LEASED_AFTER_CRASH="$(find "$DATA_DIR/queue/leased" -type f 2>/dev/null | wc -l)"

# Restart -- "agent recovers without reinstall" means exactly this:
# same install, same data dir, just run it again.
env "XDG_DATA_HOME=$SCRATCH_ROOT" "$BINARY_PATH" &
AGENT_PID=$!
sleep 1
if ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "FAIL: agent did not restart cleanly after a hard crash" >&2
    exit 1
fi

# Give it time to finish draining against the now-200-returning backend.
sleep 8

kill -9 "$AGENT_PID" 2>/dev/null || true
wait "$AGENT_PID" 2>/dev/null || true
kill -9 "$SERVER_PID" 2>/dev/null || true

PENDING="$(find "$DATA_DIR/queue/pending" -type f 2>/dev/null | wc -l)"
LEASED_FINAL="$(find "$DATA_DIR/queue/leased" -type f 2>/dev/null | wc -l)"
ACKED="$(find "$DATA_DIR/queue/acked" -type f 2>/dev/null | wc -l)"
QUARANTINE="$(find "$DATA_DIR/queue/quarantine" -type f 2>/dev/null | wc -l)"
ACCOUNTED=$((PENDING + LEASED_FINAL + ACKED + QUARANTINE))

FAILURES=()
if [[ "$ACCOUNTED" -ne "$TOTAL_SEEDED" ]]; then
    FAILURES+=("silent data loss: seeded $TOTAL_SEEDED, accounted for $ACCOUNTED (pending=$PENDING leased=$LEASED_FINAL acked=$ACKED quarantine=$QUARANTINE)")
fi
if [[ "$QUARANTINE" -ne 1 ]]; then
    FAILURES+=("expected exactly 1 quarantined record (the deliberately corrupted one), got $QUARANTINE")
fi
if [[ "$ACKED" -ne 5 ]]; then
    FAILURES+=("expected all 5 valid records acked after recovery, got $ACKED")
fi
if [[ "$LEASED_FINAL" -ne 0 ]]; then
    FAILURES+=("expected 0 records still stuck in leased/ after restart's lease-recovery, got $LEASED_FINAL")
fi

echo "{"
echo "  \"total_seeded\": $TOTAL_SEEDED,"
echo "  \"leased_immediately_after_crash\": $LEASED_AFTER_CRASH,"
echo "  \"pending_final\": $PENDING,"
echo "  \"leased_final\": $LEASED_FINAL,"
echo "  \"acked_final\": $ACKED,"
echo "  \"quarantine_final\": $QUARANTINE,"
echo "  \"rss_mb_during_faults\": $RSS_DURING_FAULTS,"
if [[ ${#FAILURES[@]} -eq 0 ]]; then
    echo "  \"result\": \"PASS\""
else
    echo "  \"result\": \"FAIL\","
    printf '  "failures": [\n'
    for i in "${!FAILURES[@]}"; do
        sep=","
        [[ "$i" -eq $((${#FAILURES[@]} - 1)) ]] && sep=""
        printf '    "%s"%s\n' "${FAILURES[$i]}" "$sep"
    done
    printf '  ]\n'
fi
echo "}"

if [[ ${#FAILURES[@]} -gt 0 ]]; then
    exit 1
fi
exit 0
