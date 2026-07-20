# AG-REL-001 — Windows counterpart of run.sh's compressed real
# fault-injection smoke test. Same real, short (~25s) flow: offline
# queueing, a corrupted record (quarantine), server 5xx, a 401
# rejection, recovery, a hard crash (kill -9 equivalent) mid-drain, and
# a restart without reinstall -- one continuous accounting pass proving
# no silent data loss. Not a multi-day soak test (explicit, honest
# scope compression -- see TEST_REPORT.md).

param(
    [Parameter(Mandatory = $true)]
    [string]$BinaryPath,
    [string]$SeedQueueBin
)

$ErrorActionPreference = "Stop"
if (-not $SeedQueueBin) {
    $SeedQueueBin = Join-Path (Split-Path $BinaryPath) "examples\seed_queue.exe"
}
if (-not (Test-Path $BinaryPath)) { throw "binary not found at $BinaryPath" }
if (-not (Test-Path $SeedQueueBin)) { throw "seed_queue not found at $SeedQueueBin" }
$BinaryPath = (Resolve-Path $BinaryPath).Path
$SeedQueueBin = (Resolve-Path $SeedQueueBin).Path

function Set-Utf8NoBom([string]$Path, [string]$Value) {
    [System.IO.File]::WriteAllText($Path, $Value, (New-Object System.Text.UTF8Encoding $false))
}

$scratchRoot = Join-Path $env:TEMP "growth-layer-agent-relsmoke"
if (Test-Path $scratchRoot) { Remove-Item -Recurse -Force $scratchRoot }
$dataDir = Join-Path $scratchRoot "GrowthLayerAgent"
New-Item -ItemType Directory -Force -Path (Join-Path $dataDir "queue\pending") | Out-Null

# Phased mock backend -- request 1-2 -> 500, request 3 -> 401,
# request 4+ -> 200. Same phase design as run.sh's own mock server.
$serverJob = Start-Job -ScriptBlock {
    $listener = New-Object System.Net.HttpListener
    $listener.Prefixes.Add("http://127.0.0.1:18090/")
    $listener.Start()
    $count = 0
    try {
        while ($true) {
            $ctx = $listener.GetContext()
            $count++
            if ($count -le 2) {
                $ctx.Response.StatusCode = 500
            } elseif ($count -eq 3) {
                $ctx.Response.StatusCode = 401
            } else {
                $ctx.Response.StatusCode = 200
                $body = [System.Text.Encoding]::UTF8.GetBytes("{}")
                $ctx.Response.ContentLength64 = $body.Length
                $ctx.Response.OutputStream.Write($body, 0, $body.Length)
            }
            $ctx.Response.KeepAlive = $false
            $ctx.Response.OutputStream.Close()
        }
    } finally {
        $listener.Stop()
    }
}
Start-Sleep -Milliseconds 300

$configJson = @{
    backend_url   = "http://127.0.0.1:18090"
    agent_token   = "rel-smoke-token"
    dashboard_url = "http://127.0.0.1:1"
} | ConvertTo-Json
Set-Utf8NoBom (Join-Path $dataDir "config.json") $configJson

& $SeedQueueBin (Join-Path $dataDir "queue") 5
if ($LASTEXITCODE -ne 0) { throw "seed_queue failed" }
Set-Utf8NoBom (Join-Path $dataDir "queue\pending\00000000-0000-0000-0000-000000000bad.json") "not a valid envelope, deliberately corrupted for this smoke test"
$totalSeeded = 6

$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $BinaryPath
$psi.UseShellExecute = $false
$psi.EnvironmentVariables["LOCALAPPDATA"] = $scratchRoot
$proc = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 1

if (-not (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)) {
    throw "FAIL: agent did not start"
}

Start-Sleep -Seconds 15

if (-not (Get-Process -Id $proc.Id -ErrorAction SilentlyContinue)) {
    throw "FAIL: agent crashed during server-error/unauthorized phase (should have backed off, not died)"
}
$p = Get-Process -Id $proc.Id
$rssDuringFaults = [math]::Round($p.WorkingSet64 / 1MB, 2)

Stop-Process -Id $proc.Id -Force
Start-Sleep -Milliseconds 500

$leasedAfterCrash = (Get-ChildItem (Join-Path $dataDir "queue\leased") -File -ErrorAction SilentlyContinue).Count

$proc2 = [System.Diagnostics.Process]::Start($psi)
Start-Sleep -Seconds 1
if (-not (Get-Process -Id $proc2.Id -ErrorAction SilentlyContinue)) {
    throw "FAIL: agent did not restart cleanly after a hard crash"
}

Start-Sleep -Seconds 8

Stop-Process -Id $proc2.Id -Force -ErrorAction SilentlyContinue
$serverJob | Remove-Job -Force

$pending = (Get-ChildItem (Join-Path $dataDir "queue\pending") -File -ErrorAction SilentlyContinue).Count
$leasedFinal = (Get-ChildItem (Join-Path $dataDir "queue\leased") -File -ErrorAction SilentlyContinue).Count
$acked = (Get-ChildItem (Join-Path $dataDir "queue\acked") -File -ErrorAction SilentlyContinue).Count
$quarantine = (Get-ChildItem (Join-Path $dataDir "queue\quarantine") -File -ErrorAction SilentlyContinue).Count
$accounted = $pending + $leasedFinal + $acked + $quarantine

$failures = @()
if ($accounted -ne $totalSeeded) {
    $failures += "silent data loss: seeded $totalSeeded, accounted for $accounted (pending=$pending leased=$leasedFinal acked=$acked quarantine=$quarantine)"
}
if ($quarantine -ne 1) {
    $failures += "expected exactly 1 quarantined record, got $quarantine"
}
if ($acked -ne 5) {
    $failures += "expected all 5 valid records acked after recovery, got $acked"
}
if ($leasedFinal -ne 0) {
    $failures += "expected 0 records still stuck in leased/ after restart, got $leasedFinal"
}

$report = [ordered]@{
    total_seeded                 = $totalSeeded
    leased_immediately_after_crash = $leasedAfterCrash
    pending_final                 = $pending
    leased_final                  = $leasedFinal
    acked_final                   = $acked
    quarantine_final              = $quarantine
    rss_mb_during_faults          = $rssDuringFaults
    result                        = if ($failures.Count -eq 0) { "PASS" } else { "FAIL" }
    failures                      = $failures
}
$report | ConvertTo-Json

Remove-Item -Recurse -Force $scratchRoot -ErrorAction SilentlyContinue

if ($failures.Count -gt 0) { exit 1 }
exit 0
