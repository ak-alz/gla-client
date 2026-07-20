#!/usr/bin/env bash
# AG-PERF-001 — Linux resource-budget benchmark harness. Mirrors run.ps1
# scenario-for-scenario (idle/active/offline_queue/upload_burst) and reuses
# the same budgets.json, but measures via /proc (as AG-LNX-002/AG-LNX-003
# already did for this project's Linux collector work) instead of
# Get-Process: /proc/<pid>/status's VmRSS for RSS, /proc/<pid>/stat's
# utime+stime (clock ticks, converted via `getconf CLK_TCK`) delta over
# wall-clock delta for CPU%.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCENARIO="${1:?usage: run.sh <idle|active|offline_queue|upload_burst|update_download|update_apply> [binary_path] [duration_seconds] [sample_interval_seconds]}"

case "$SCENARIO" in
    idle|active|offline_queue|upload_burst|update_download|update_apply) ;;
    *) echo "unknown scenario '$SCENARIO'" >&2; exit 2 ;;
esac

# update_download/update_apply measure a different real binary entirely
# (updater's own examples/update_bench.rs, exercising
# download_with_checksum/Staging::stage_and_swap+commit for real) -- not
# growth-layer-agent, which never runs these code paths on demand within
# a short benchmark window (real update checks are scheduled, not
# immediate). Same real reasoning as the Windows harness's own split.
IS_UPDATE_BENCH_SCENARIO=0
if [[ "$SCENARIO" == "update_download" || "$SCENARIO" == "update_apply" ]]; then
    IS_UPDATE_BENCH_SCENARIO=1
fi
if [[ "$IS_UPDATE_BENCH_SCENARIO" -eq 1 ]]; then
    DEFAULT_BINARY_PATH="$SCRIPT_DIR/../../target/release/examples/update_bench"
else
    DEFAULT_BINARY_PATH="$SCRIPT_DIR/../../target/release/growth-layer-agent"
fi
BINARY_PATH="${2:-$DEFAULT_BINARY_PATH}"
DURATION_SECONDS="${3:-14}"
# upload_burst drains its whole backlog in well under a 2s cadence (the
# same real finding made on the Windows harness) -- sample much finer by
# default so the burst itself is actually captured.
if [[ "$SCENARIO" == "upload_burst" ]]; then
    SAMPLE_INTERVAL_SECONDS="${4:-0.1}"
else
    SAMPLE_INTERVAL_SECONDS="${4:-2}"
fi

if [[ ! -x "$BINARY_PATH" ]]; then
    echo "binary not found at $BINARY_PATH -- build it first: cargo build --release -p agent-bin (or --example update_bench -p updater)" >&2
    exit 2
fi
BINARY_PATH="$(cd "$(dirname "$BINARY_PATH")" && pwd)/$(basename "$BINARY_PATH")"

# Derived from BINARY_PATH's own directory rather than a fixed path
# relative to this script: a custom CARGO_TARGET_DIR (this project's own
# convention for Linux builds, keeping them out of the Windows-built
# target/ this repo checkout also contains) means the release directory
# is not always $SCRIPT_DIR/../../target/release.
SEED_QUEUE_BIN="$(dirname "$BINARY_PATH")/examples/seed_queue"

BUDGET_JSON="$(python3 -c "
import json, sys
with open('$SCRIPT_DIR/budgets.json') as f:
    budgets = json.load(f)
budget = budgets.get('$SCENARIO')
if budget is None:
    print('no budget defined for scenario $SCENARIO', file=sys.stderr)
    sys.exit(1)
print(json.dumps(budget))
")"

# Mirrors paths::data_dir()'s real Linux resolution ($XDG_DATA_HOME/
# growth-layer-agent) exactly -- the agent process gets XDG_DATA_HOME
# pointed at SCRATCH_ROOT below, so it independently derives the same
# DATA_DIR this script writes into.
SCRATCH_ROOT="$(mktemp -d -t growth-layer-agent-perfbench-XXXXXX)"
DATA_DIR="$SCRATCH_ROOT/growth-layer-agent"
mkdir -p "$DATA_DIR"

cleanup() {
    [[ -n "${ACK_SERVER_PID:-}" ]] && kill -9 "$ACK_SERVER_PID" 2>/dev/null || true
    [[ -n "${AGENT_PID:-}" ]] && kill -9 "$AGENT_PID" 2>/dev/null || true
    [[ -n "${XTERM_PID:-}" ]] && kill -9 "$XTERM_PID" 2>/dev/null || true
    rm -rf "$SCRATCH_ROOT"
}
trap cleanup EXIT

BACKEND_URL="http://127.0.0.1:1"
if [[ "$SCENARIO" == "upload_burst" ]]; then
    # A real, minimal, self-contained HTTP server -- genuinely accepts and
    # acks every request with 200, one connection at a time, for the whole
    # scenario duration. `Connection: close` on every response (the same
    # real bug found on the Windows harness: a client reusing a keep-alive
    # connection this minimal server doesn't handle hangs until the
    # uploader's own transport timeout, turning a 40-record burst into
    # ~1 acked record per run). Binds an OS-assigned ephemeral port (port
    # 0), not a fixed 18080 -- a real run-to-run "Address already in use"
    # race was found against a fixed port (a prior run's socket lingering
    # in TIME_WAIT), the same reason update_bench.rs's own mock server
    # already binds ephemerally instead of to a fixed port.
    ACK_PORT_FILE="$SCRATCH_ROOT/ack_server_port"
    python3 -c "
import http.server
class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get('Content-Length', 0))
        self.rfile.read(length)
        body = b'{}'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.send_header('Connection', 'close')
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass
srv = http.server.HTTPServer(('127.0.0.1', 0), Handler)
with open('$ACK_PORT_FILE', 'w') as f:
    f.write(str(srv.server_address[1]))
srv.serve_forever()
" &
    ACK_SERVER_PID=$!
    for _ in $(seq 1 50); do
        [[ -s "$ACK_PORT_FILE" ]] && break
        sleep 0.1
    done
    ACK_PORT="$(cat "$ACK_PORT_FILE" 2>/dev/null)"
    if [[ -z "$ACK_PORT" ]]; then
        echo "mock ack server never reported its bound port" >&2
        exit 1
    fi
    BACKEND_URL="http://127.0.0.1:$ACK_PORT"
fi

if [[ "$IS_UPDATE_BENCH_SCENARIO" -eq 0 ]]; then
    cat > "$DATA_DIR/config.json" <<JSON
{"backend_url":"$BACKEND_URL","agent_token":"perf-bench-token","dashboard_url":"http://127.0.0.1:1"}
JSON
fi

QUEUE_ACKED=0
QUEUE_REMAINING_PENDING=0
QUEUE_LEASED=0
QUEUE_QUARANTINE=0
if [[ "$SCENARIO" == "upload_burst" ]]; then
    if [[ ! -x "$SEED_QUEUE_BIN" ]]; then
        echo "seed_queue example not built -- run: cargo build --release --example seed_queue -p durable-queue" >&2
        exit 2
    fi
    "$SEED_QUEUE_BIN" "$DATA_DIR/queue" 40
fi

BINARY_ARGS=()
if [[ "$SCENARIO" == "update_download" ]]; then
    # 6MB at a 2MB/s cap (~3s) -- the real growth-layer-agent binary is
    # ~2-3MB on both platforms (checked directly), so this is a
    # representative artifact size: an earlier 20MB choice inflated this
    # benchmark's own measured RSS with artifact-buffer bytes unrelated to
    # the downloader's actual overhead, which is what this budget bounds.
    BINARY_ARGS=(download 6000000 2000000)
elif [[ "$SCENARIO" == "update_apply" ]]; then
    BINARY_ARGS=(apply)
fi

# A real X11 session -- WSLg's own DISPLAY=:0 for a local run (the same
# live desktop AG-LNX-002/AG-LNX-003 already validated the X11
# active-window backend against, not a headless Xvfb standing in for
# one), or whatever DISPLAY the caller already exported (e.g. CI running
# this under `xvfb-run`, which picks its own display number) -- honoring
# an already-set $DISPLAY rather than hardcoding :0 is what makes this
# scenario portable to a CI runner with no WSLg.
BENCH_DISPLAY="${DISPLAY:-:0}"
ENV_PREFIX=(env "XDG_DATA_HOME=$SCRATCH_ROOT")
if [[ "$SCENARIO" == "active" ]]; then
    ENV_PREFIX+=("DISPLAY=$BENCH_DISPLAY" "XDG_SESSION_TYPE=x11")
fi

"${ENV_PREFIX[@]}" "$BINARY_PATH" "${BINARY_ARGS[@]}" &
AGENT_PID=$!
sleep 0.5

if [[ "$IS_UPDATE_BENCH_SCENARIO" -eq 0 ]] && ! kill -0 "$AGENT_PID" 2>/dev/null; then
    echo "agent process exited immediately -- check $DATA_DIR for logs" >&2
    exit 1
fi

if [[ "$SCENARIO" == "active" ]]; then
    DISPLAY="$BENCH_DISPLAY" xterm -geometry 80x24+0+0 &
    XTERM_PID=$!
    sleep 0.8
    XTERM_WINDOW_ID="$(DISPLAY="$BENCH_DISPLAY" xdotool search --sync --pid "$XTERM_PID" 2>/dev/null | head -1)"
    [[ -n "$XTERM_WINDOW_ID" ]] && DISPLAY="$BENCH_DISPLAY" xdotool windowactivate "$XTERM_WINDOW_ID" 2>/dev/null || true
fi

CLK_TCK="$(getconf CLK_TCK)"
START_EPOCH="$(date +%s.%N)"
PREV_TICKS=""
PREV_TIME="$START_EPOCH"
SAMPLES=()
CPU_SAMPLES=()

read_rss_mb() {
    awk '/^VmRSS:/ { printf "%.2f", $2/1024 }' "/proc/$1/status" 2>/dev/null || true
}
read_cpu_ticks() {
    awk '{ print $14+$15 }' "/proc/$1/stat" 2>/dev/null || true
}
# Real bytes actually written to storage (not just to the page cache) --
# the same field `write_bytes` documented in `proc(5)`, direct and
# reliable, no extra tooling needed (unlike Windows, which needed a
# P/Invoke of GetProcessIoCounters since .NET's own Process class doesn't
# expose this).
read_write_bytes() {
    # `|| true` -- update_download/update_apply's own process can exit
    # (naturally, having finished its real work) before this is read,
    # unlike the long-lived growth-layer-agent every other scenario
    # measures; awk's exit status for "no such file" would otherwise trip
    # this script's own `set -e` and abort the whole run (a real bug
    # found the first time these two scenarios were exercised with this
    # measurement in place, not assumed).
    awk '/^write_bytes:/ { print $2 }' "/proc/$1/io" 2>/dev/null || true
}

WRITE_BYTES_START="$(read_write_bytes "$AGENT_PID")"
[[ -z "$WRITE_BYTES_START" ]] && WRITE_BYTES_START=0

while true; do
    NOW="$(date +%s.%N)"
    ELAPSED="$(python3 -c "print($NOW - $START_EPOCH)")"
    if (( $(python3 -c "print(1 if $ELAPSED >= $DURATION_SECONDS else 0)") )); then
        break
    fi

    if [[ "$SCENARIO" == "active" && -n "${XTERM_WINDOW_ID:-}" ]]; then
        DISPLAY="$BENCH_DISPLAY" xdotool type --window "$XTERM_WINDOW_ID" "the quick brown fox " 2>/dev/null || true
    fi

    sleep "$SAMPLE_INTERVAL_SECONDS"
    if ! kill -0 "$AGENT_PID" 2>/dev/null; then break; fi

    RSS="$(read_rss_mb "$AGENT_PID")"
    [[ -n "$RSS" ]] && SAMPLES+=("$RSS")

    TICKS="$(read_cpu_ticks "$AGENT_PID")"
    SAMPLE_TIME="$(date +%s.%N)"
    if [[ -n "$TICKS" && -n "$PREV_TICKS" ]]; then
        CPU_PCT="$(python3 -c "
ticks_delta = $TICKS - $PREV_TICKS
wall_delta = $SAMPLE_TIME - $PREV_TIME
print(round((ticks_delta / $CLK_TCK / wall_delta) * 100.0, 3) if wall_delta > 0 else 0)
")"
        CPU_SAMPLES+=("$CPU_PCT")
    fi
    PREV_TICKS="$TICKS"
    PREV_TIME="$SAMPLE_TIME"
done

# Read one last time before the process is killed -- /proc/<pid>/io
# disappears the moment it exits.
WRITE_BYTES_END="$(read_write_bytes "$AGENT_PID")"
[[ -z "$WRITE_BYTES_END" ]] && WRITE_BYTES_END="$WRITE_BYTES_START"
ACTUAL_WINDOW_SECONDS="$(python3 -c "print($(date +%s.%N) - $START_EPOCH)")"

kill -9 "$AGENT_PID" 2>/dev/null || true
wait "$AGENT_PID" 2>/dev/null || true
if [[ -n "${XTERM_PID:-}" ]]; then kill -9 "$XTERM_PID" 2>/dev/null || true; fi
if [[ -n "${ACK_SERVER_PID:-}" ]]; then kill -9 "$ACK_SERVER_PID" 2>/dev/null || true; fi

if [[ "$SCENARIO" == "upload_burst" ]]; then
    QUEUE_REMAINING_PENDING="$(find "$DATA_DIR/queue/pending" -type f 2>/dev/null | wc -l)"
    QUEUE_ACKED="$(find "$DATA_DIR/queue/acked" -type f 2>/dev/null | wc -l)"
    QUEUE_LEASED="$(find "$DATA_DIR/queue/leased" -type f 2>/dev/null | wc -l)"
    QUEUE_QUARANTINE="$(find "$DATA_DIR/queue/quarantine" -type f 2>/dev/null | wc -l)"
fi

REPORT="$(python3 -c "
import json
samples = [$(IFS=,; echo "${SAMPLES[*]:-}")] if '$(IFS=,; echo "${SAMPLES[*]:-}")' else []
cpu_samples = [$(IFS=,; echo "${CPU_SAMPLES[*]:-}")] if '$(IFS=,; echo "${CPU_SAMPLES[*]:-}")' else []
budget = json.loads('''$BUDGET_JSON''')

rss_max = max(samples) if samples else 0
cpu_avg = sum(cpu_samples)/len(cpu_samples) if cpu_samples else 0
cpu_p95 = 0
if cpu_samples:
    s = sorted(cpu_samples)
    idx = min(len(s)-1, max(0, -(-int(0.95*len(s))) - 1))
    cpu_p95 = s[idx]

# Projected from this scenario's real (short) measured window out to an
# hourly rate -- see budgets.json's own note on why this is an explicit
# extrapolation, not a full hour actually measured.
write_bytes_delta = $WRITE_BYTES_END - $WRITE_BYTES_START
actual_window_seconds = $ACTUAL_WINDOW_SECONDS
disk_write_kb_per_hour = round((write_bytes_delta / 1024) * (3600.0 / actual_window_seconds), 2) if actual_window_seconds > 0 else 0

violations = []
if budget.get('rss_mb_max') and rss_max > budget['rss_mb_max']:
    violations.append(f'RSS {rss_max}MB exceeds budget {budget[\"rss_mb_max\"]}MB')
if budget.get('cpu_percent_avg_max') and cpu_avg > budget['cpu_percent_avg_max']:
    violations.append(f'CPU avg {cpu_avg}% exceeds budget {budget[\"cpu_percent_avg_max\"]}%')
if budget.get('cpu_percent_p95_max') and cpu_p95 > budget['cpu_percent_p95_max']:
    violations.append(f'CPU p95 {cpu_p95}% exceeds budget {budget[\"cpu_percent_p95_max\"]}%')
if budget.get('disk_write_kb_per_hour_max') and disk_write_kb_per_hour > budget['disk_write_kb_per_hour_max']:
    violations.append(f'disk writes (projected) {disk_write_kb_per_hour}KB/hour exceeds budget {budget[\"disk_write_kb_per_hour_max\"]}KB/hour')

report = {
    'scenario': '$SCENARIO',
    'platform': 'linux',
    'rss_mb_max': rss_max,
    'cpu_percent_avg': cpu_avg,
    'cpu_percent_p95': cpu_p95,
    'disk_write_kb_per_hour_projected': disk_write_kb_per_hour,
    'samples': len(samples),
    'queue_acked': $QUEUE_ACKED,
    'queue_remaining_pending': $QUEUE_REMAINING_PENDING,
    'queue_leased': $QUEUE_LEASED,
    'queue_quarantine': $QUEUE_QUARANTINE,
    'violations': violations,
}
print(json.dumps(report, indent=2))
print('VIOLATIONS_COUNT=' + str(len(violations)))
" > "$SCRATCH_ROOT/report.txt")"

VIOLATIONS_COUNT="$(grep -o 'VIOLATIONS_COUNT=.*' "$SCRATCH_ROOT/report.txt" | cut -d= -f2)"
grep -v 'VIOLATIONS_COUNT=' "$SCRATCH_ROOT/report.txt"

if [[ "$VIOLATIONS_COUNT" -gt 0 ]]; then
    exit 1
fi
exit 0
