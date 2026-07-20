# AG-PERF-001 — Windows resource-budget benchmark harness. Reuses the
# exact measurement method already validated throughout this project
# (AG-002's own benchmark, AG-WIN-001's collector measurement):
# Get-Process's WorkingSet64 for RSS, cumulative CPU seconds delta over
# wall-clock elapsed time for CPU%. Not a new measurement technique —
# consistency with everything already measured this way matters more
# than trying a different one now.

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("idle", "active", "offline_queue", "upload_burst", "update_download", "update_apply")]
    [string]$Scenario,
    [string]$BinaryPath,
    [int]$DurationSeconds = 14,
    [double]$SampleIntervalSeconds = 2
)

# update_download/update_apply measure a different real binary entirely
# (updater's own `examples/update_bench.rs`, exercising
# `download_with_checksum`/`Staging::stage_and_swap`+`commit` for real) --
# not growth-layer-agent, which never runs these code paths on demand
# within a short benchmark window (real update checks are scheduled, not
# immediate).
$isUpdateBenchScenario = $Scenario -in @("update_download", "update_apply")
if (-not $BinaryPath) {
    $BinaryPath = if ($isUpdateBenchScenario) {
        "$PSScriptRoot\..\..\target\release\examples\update_bench.exe"
    } else {
        "$PSScriptRoot\..\..\target\release\growth-layer-agent.exe"
    }
}

# upload_burst drains its whole 40-record backlog in well under 2 seconds
# (found by a real run: the default cadence's first sample landed AFTER the
# burst finished, reporting a misleading 0% CPU average for a scenario whose
# entire point is CPU during the burst) -- sample much finer by default so
# the burst itself is actually captured, unless the caller explicitly
# overrode the interval.
if ($Scenario -eq "upload_burst" -and -not $PSBoundParameters.ContainsKey("SampleIntervalSeconds")) {
    $SampleIntervalSeconds = 0.1
}

$ErrorActionPreference = "Stop"

# Windows PowerShell 5.1's Set-Content has no `utf8NoBOM` option (only
# added in PowerShell 6+/Core) — its plain `UTF8` always emits a BOM,
# which real found-by-running-this-script bug: a BOM-prefixed
# QUEUE_FORMAT_VERSION file makes durable-queue's version string
# comparison fail (`"﻿2" != "2"`). Writing via the .NET encoding
# object directly (`$false` = no BOM) is the standard 5.1 workaround.
function Set-Utf8NoBom([string]$Path, [string]$Value) {
    [System.IO.File]::WriteAllText($Path, $Value, (New-Object System.Text.UTF8Encoding $false))
}

# Real cumulative disk-write bytes via the actual Win32 IO counters
# (GetProcessIoCounters) -- .NET's own Process class exposes CPU/RSS but
# not IO byte counts, and Get-Counter's per-instance-name matching is
# unreliable when multiple same-named processes exist. P/Invoke is the
# direct, unambiguous (keyed by real handle, not a guessed instance name)
# way to get this real number.
Add-Type -Namespace PerfBench -Name IoCounters -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError = true)]
public static extern bool GetProcessIoCounters(System.IntPtr hProcess, out IO_COUNTERS lpIoCounters);

[System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
public struct IO_COUNTERS {
    public ulong ReadOperationCount;
    public ulong WriteOperationCount;
    public ulong OtherOperationCount;
    public ulong ReadTransferCount;
    public ulong WriteTransferCount;
    public ulong OtherTransferCount;
}
'@

function Get-ProcessWriteBytes([System.Diagnostics.Process]$Process) {
    $counters = New-Object PerfBench.IoCounters+IO_COUNTERS
    if ([PerfBench.IoCounters]::GetProcessIoCounters($Process.Handle, [ref]$counters)) {
        return [uint64]$counters.WriteTransferCount
    }
    return $null
}

$budgets = Get-Content (Join-Path $PSScriptRoot "budgets.json") -Raw | ConvertFrom-Json
$budget = $budgets.$Scenario
if ($null -eq $budget) { throw "no budget defined for scenario '$Scenario'" }

if (-not (Test-Path $BinaryPath)) {
    throw "binary not found at $BinaryPath -- build it first: cargo build --release -p agent-bin"
}
$BinaryPath = (Resolve-Path $BinaryPath).Path

# Isolated per-scenario data dir via a LOCALAPPDATA override for the
# CHILD process only (this process's own $env:LOCALAPPDATA is restored
# at the end) -- never touches the real user's actual agent data dir.
$scratchRoot = Join-Path $env:TEMP "growth-layer-agent-perfbench-$Scenario"
if (Test-Path $scratchRoot) { Remove-Item -Recurse -Force $scratchRoot }
New-Item -ItemType Directory -Force -Path (Join-Path $scratchRoot "GrowthLayerAgent") | Out-Null

$dataDir = Join-Path $scratchRoot "GrowthLayerAgent"
# Deliberately unreachable (nothing listens on this loopback port) for
# every scenario except upload_burst -- offline backoff behavior must
# not differ between idle/active/offline_queue, only upload_burst gets
# a real, reachable, acking endpoint.
$backendUrl = "http://127.0.0.1:1"
if ($Scenario -eq "upload_burst") {
    # A real, minimal, native HttpListener, entirely self-contained
    # inside a background job -- genuinely accepts and acks POST
    # requests like the real backend's ingest endpoint would (200 OK,
    # empty JSON body is enough for uploader::classify_status's 2xx
    # path), CONTINUOUSLY for the whole scenario duration, decoupled
    # from the main script's sampling-loop cadence (an earlier version
    # only checked for one pending request per 2-second sample tick,
    # which throttled the "burst" down to a trickle -- a real bug found
    # by actually inspecting the queue's post-run state, not assumed).
    $ackJob = Start-Job -ScriptBlock {
        $listener = New-Object System.Net.HttpListener
        $listener.Prefixes.Add("http://127.0.0.1:18080/")
        $listener.Start()
        try {
            while ($true) {
                $ctx = $listener.GetContext() # blocks until a request arrives
                $body = [System.Text.Encoding]::UTF8.GetBytes("{}")
                $ctx.Response.StatusCode = 200
                $ctx.Response.ContentLength64 = $body.Length
                # Found by a real run: without this, the client's 2nd
                # request on a reused keep-alive connection hung until the
                # uploader's 10s transport timeout, one NetworkError at a
                # time -- turning a 40-record burst into ~1 acked record per
                # scenario run. Closing the connection after every response
                # forces the client to open a fresh one for the next
                # request, which this minimal listener handles cleanly.
                $ctx.Response.KeepAlive = $false
                $ctx.Response.OutputStream.Write($body, 0, $body.Length)
                $ctx.Response.OutputStream.Close()
            }
        } finally {
            $listener.Stop()
        }
    }
    $backendUrl = "http://127.0.0.1:18080"
}

if (-not $isUpdateBenchScenario) {
    $configJson = @{
        backend_url    = $backendUrl
        agent_token    = "perf-bench-token"
        dashboard_url  = "http://127.0.0.1:1"
    } | ConvertTo-Json
    Set-Utf8NoBom (Join-Path $dataDir "config.json") $configJson
}

if ($Scenario -eq "upload_burst") {
    # Pre-populate the queue with real, valid envelope files BEFORE launch
    # so the agent has a real backlog to drain the moment its uploader
    # thread starts. An earlier version of this block hand-wrote
    # approximated JSON directly -- every one of those 40 records ended up
    # in quarantine/ because the hand-guessed shape didn't match what
    # `Envelope::build_or_quarantine`/`DurableQueue::enqueue` actually
    # produce (found by inspecting the post-run queue state, not assumed).
    # Fixed by seeding through the real API instead of guessing its wire
    # format -- see `durable-queue/examples/seed_queue.rs`.
    $seedQueueBin = "$PSScriptRoot\..\..\target\release\examples\seed_queue.exe"
    if (-not (Test-Path $seedQueueBin)) {
        throw "seed_queue example not built -- run: cargo build --release --example seed_queue -p durable-queue"
    }
    $queueDir = Join-Path $dataDir "queue"
    & $seedQueueBin $queueDir 40
    if ($LASTEXITCODE -ne 0) { throw "seed_queue failed with exit code $LASTEXITCODE" }
}

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $BinaryPath
$psi.UseShellExecute = $false
if ($Scenario -eq "update_download") {
    # 6MB at a 2MB/s cap (~3s) -- the real growth-layer-agent binary is
    # ~2-3MB on both platforms (checked directly), so this is a
    # representative artifact size, not an arbitrary round number: an
    # earlier 20MB choice inflated this benchmark's own measured RSS with
    # artifact-buffer bytes that have nothing to do with the downloader's
    # actual overhead, which is what this budget is meant to bound (see
    # budgets.json's own `_source` note for update_download).
    # `.Arguments` (a plain string), not `.ArgumentList` -- this Windows
    # PowerShell 5.1's ProcessStartInfo left the latter null by default
    # (a real difference found by running this exact line, not assumed
    # from newer-.NET documentation).
    $psi.Arguments = "download 6000000 2000000"
} elseif ($Scenario -eq "update_apply") {
    $psi.Arguments = "apply"
} else {
    $psi.EnvironmentVariables["LOCALAPPDATA"] = $scratchRoot
}
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Milliseconds 500

if (-not $isUpdateBenchScenario -and -not (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)) {
    throw "agent process exited immediately -- check $dataDir\logs for details"
}

$writeBytesStart = Get-ProcessWriteBytes -Process $proc

if ($Scenario -eq "active") {
    Add-Type -AssemblyName System.Windows.Forms
    Start-Process notepad.exe
    Start-Sleep -Milliseconds 800
}

$samples = @()
$cpuSamples = @()
$startTime = Get-Date
$prevCpu = $null
$prevTime = $startTime

while (((Get-Date) - $startTime).TotalSeconds -lt $DurationSeconds) {
    if ($Scenario -eq "active") {
        [System.Windows.Forms.SendKeys]::SendWait("the quick brown fox jumps over the lazy dog ")
    }

    Start-Sleep -Seconds $SampleIntervalSeconds
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($null -eq $p) { break }
    $now = Get-Date
    $rssMb = [math]::Round($p.WorkingSet64 / 1MB, 2)
    $samples += $rssMb

    if ($null -ne $prevCpu) {
        $cpuDeltaSeconds = ($p.TotalProcessorTime - $prevCpu).TotalSeconds
        $wallDeltaSeconds = ($now - $prevTime).TotalSeconds
        $cpuPercent = [math]::Round(($cpuDeltaSeconds / $wallDeltaSeconds) * 100.0, 3)
        $cpuSamples += $cpuPercent
    }
    $prevCpu = $p.TotalProcessorTime
    $prevTime = $now
}

# Read one last time before the process is killed -- the handle becomes
# invalid the moment it exits.
$finalProcess = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
$writeBytesEnd = if ($finalProcess) { Get-ProcessWriteBytes -Process $finalProcess } else { $writeBytesStart }

# `update_download`/`update_apply`'s own exit code was never actually
# checked here -- an independent review found this: a real regression in
# `download_with_checksum`/`Staging` (checksum mismatch, panic, disk
# full) would be completely invisible to this harness, since only RSS/CPU
# were compared against budgets regardless of whether the real work
# underneath actually succeeded. Checked BEFORE `Stop-Process -Force`
# below -- a follow-up review found that ordering it AFTER made the
# "still running" branch dead code (Stop-Process's own SIGKILL-equivalent
# had already forced an exit by the time this ran, so a real hang always
# reported a fake exit code like -1 instead of a clear timeout message).
$updateBenchViolation = $null
if ($isUpdateBenchScenario) {
    $proc.WaitForExit(5000) | Out-Null
    if ($proc.HasExited -and $proc.ExitCode -ne 0) {
        $updateBenchViolation = "update_bench exited with code $($proc.ExitCode) (expected 0)"
    } elseif (-not $proc.HasExited) {
        $updateBenchViolation = "update_bench did not exit within 5s of the scenario window ending"
    }
}

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
if ($Scenario -eq "upload_burst") {
    # `Stop-Job` first hangs here -- the job is blocked inside a
    # synchronous `HttpListener.GetContext()` call, which doesn't
    # cooperatively respond to PowerShell's stop signal. `Remove-Job
    # -Force` alone forcibly tears down the job's underlying process
    # regardless of what it's blocked on (found by this exact hang
    # happening on the first real run, not assumed).
    $ackJob | Remove-Job -Force
}

$queueRemainingPending = 0
$queueAcked = 0
$queueLeased = 0
$queueQuarantine = 0
if ($Scenario -eq "upload_burst") {
    $pendingDir = Join-Path $dataDir "queue\pending"
    $ackedDir = Join-Path $dataDir "queue\acked"
    $leasedDir = Join-Path $dataDir "queue\leased"
    $quarantineDir = Join-Path $dataDir "queue\quarantine"
    if (Test-Path $pendingDir) { $queueRemainingPending = (Get-ChildItem $pendingDir -File -ErrorAction SilentlyContinue).Count }
    if (Test-Path $ackedDir) { $queueAcked = (Get-ChildItem $ackedDir -File -ErrorAction SilentlyContinue).Count }
    if (Test-Path $leasedDir) { $queueLeased = (Get-ChildItem $leasedDir -File -ErrorAction SilentlyContinue).Count }
    if (Test-Path $quarantineDir) { $queueQuarantine = (Get-ChildItem $quarantineDir -File -ErrorAction SilentlyContinue).Count }
}

$rssMax = if ($samples.Count -gt 0) { ($samples | Measure-Object -Maximum).Maximum } else { 0 }
$cpuAvg = if ($cpuSamples.Count -gt 0) { ($cpuSamples | Measure-Object -Average).Average } else { 0 }
$cpuP95 = if ($cpuSamples.Count -gt 0) {
    $sorted = $cpuSamples | Sort-Object
    $idx = [math]::Min($sorted.Count - 1, [int][math]::Ceiling(0.95 * $sorted.Count) - 1)
    $sorted[$idx]
} else { 0 }

# Projected from this scenario's real (short) measured window out to an
# hourly rate -- see budgets.json's own note on why this is an explicit
# extrapolation, not a full hour actually measured.
$actualWindowSeconds = ((Get-Date) - $startTime).TotalSeconds
$diskWriteBytesDelta = if ($null -ne $writeBytesEnd -and $null -ne $writeBytesStart) { [double]$writeBytesEnd - [double]$writeBytesStart } else { 0 }
$diskWriteKbPerHour = if ($actualWindowSeconds -gt 0) { [math]::Round(($diskWriteBytesDelta / 1KB) * (3600.0 / $actualWindowSeconds), 2) } else { 0 }

$violations = @()
if ($budget.rss_mb_max -and $rssMax -gt $budget.rss_mb_max) {
    $violations += "RSS ${rssMax}MB exceeds budget $($budget.rss_mb_max)MB"
}
if ($budget.cpu_percent_avg_max -and $cpuAvg -gt $budget.cpu_percent_avg_max) {
    $violations += "CPU avg ${cpuAvg}% exceeds budget $($budget.cpu_percent_avg_max)%"
}
if ($budget.cpu_percent_p95_max -and $cpuP95 -gt $budget.cpu_percent_p95_max) {
    $violations += "CPU p95 ${cpuP95}% exceeds budget $($budget.cpu_percent_p95_max)%"
}
if ($budget.disk_write_kb_per_hour_max -and $diskWriteKbPerHour -gt $budget.disk_write_kb_per_hour_max) {
    $violations += "disk writes (projected) ${diskWriteKbPerHour}KB/hour exceeds budget $($budget.disk_write_kb_per_hour_max)KB/hour"
}
if ($updateBenchViolation) {
    $violations += $updateBenchViolation
}
# `upload_burst`'s whole point is draining the seeded backlog -- an
# independent review found that a real partial-drain regression (records
# stuck in `leased/`, or quarantined) was never actually asserted against
# the expected count, only reported informationally.
if ($Scenario -eq "upload_burst" -and ($queueAcked -ne 40 -or $queueQuarantine -gt 0 -or $queueLeased -gt 0)) {
    $violations += "upload_burst did not fully drain: acked=$queueAcked (expected 40), quarantine=$queueQuarantine, leased=$queueLeased"
}

$report = [ordered]@{
    scenario       = $Scenario
    platform       = "windows"
    rss_mb_max     = $rssMax
    cpu_percent_avg = $cpuAvg
    cpu_percent_p95 = $cpuP95
    disk_write_kb_per_hour_projected = $diskWriteKbPerHour
    samples        = $samples.Count
    queue_acked    = $queueAcked
    queue_remaining_pending = $queueRemainingPending
    queue_leased   = $queueLeased
    queue_quarantine = $queueQuarantine
    violations     = $violations
}
$report | ConvertTo-Json

Remove-Item -Recurse -Force $scratchRoot -ErrorAction SilentlyContinue

if ($violations.Count -gt 0) {
    Write-Error "BUDGET VIOLATION(S): $($violations -join '; ')"
    exit 1
}
exit 0
