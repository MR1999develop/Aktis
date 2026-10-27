# Local checks on Windows: format, clippy, tests, release build.
#
# Everything is shown on screen while it runs and written to target\check.log.
# target\check-summary.txt gets, for each step, its status and time and the
# lines that matter (errors, warnings with their location, failed tests,
# panics, format differences), plus the environment the checks ran in.
# The summary is what Claude reads to fix problems.
#
#   scripts\check.bat            all steps
#   scripts\check.bat -Quick     without the release build
#   scripts\check.bat -Step test only one step (fmt, clippy, test, build)
param(
    [switch]$Quick,
    [ValidateSet('', 'fmt', 'clippy', 'test', 'build')][string]$Step = ''
)
$ErrorActionPreference = 'Continue'
Set-Location (Split-Path $PSScriptRoot)
New-Item -ItemType Directory -Force target | Out-Null

$logPath = Join-Path (Get-Location) 'target\check.log'
$sumPath = Join-Path (Get-Location) 'target\check-summary.txt'
$log = [System.IO.StreamWriter]::new($logPath, $false, [System.Text.UTF8Encoding]::new($false))
$log.AutoFlush = $true
$summary = [System.Collections.Generic.List[string]]::new()
$started = Get-Date
$env:RUST_BACKTRACE = '1'   # panics in tests show where they happened

function Say([string]$text, [string]$color = '') {
    if ($color) { Write-Host $text -ForegroundColor $color } else { Write-Host $text }
    $log.WriteLine($text)
}

function Note([string]$text) { $summary.Add($text) }

# Output of a short command, or why it could not run.
function Probe([string]$exe, [string[]]$arguments) {
    try { ((& $exe @arguments 2>&1) | ForEach-Object { "$_" }) -join ' ' } catch { "not available ($($_.Exception.Message))" }
}

Say "check started $started" 'Cyan'
Note "check started $started"

# ---- Environment -----------------------------------------------------------
Note ''
Note '== environment'
$head = Probe 'git' @('log', '-1', '--format=%h %s')
$branch = Probe 'git' @('rev-parse', '--abbrev-ref', 'HEAD')
$dirty = @(& git status --porcelain 2>$null).Count
Note "git:      $branch @ $head ($dirty changed or untracked files)"
Note "rustc:    $(Probe 'rustc' @('-V'))"
Note "cargo:    $(Probe 'cargo' @('-V'))"
Note "windows:  $([System.Environment]::OSVersion.VersionString)"
try {
    $gpus = (Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) (driver $($_.DriverVersion))" }) -join '; '
    Note "gpu:      $gpus"
} catch { Note 'gpu:      unknown' }
$dataDir = if ($env:RNE_DATA_DIR) { $env:RNE_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'raw-node-editor' }
$modelsDir = if ($env:RNE_MODELS_DIR) { $env:RNE_MODELS_DIR } else { Join-Path $dataDir 'models' }
$ortCandidates = @($env:ORT_DYLIB_PATH, 'target\release\onnxruntime.dll', 'target\debug\onnxruntime.dll',
    (Join-Path $modelsDir 'onnxruntime.dll'), (Join-Path $dataDir 'onnxruntime.dll')) | Where-Object { $_ }
$ort = $ortCandidates | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
if ($ort) {
    $ver = (Get-Item $ort).VersionInfo.ProductVersion
    Note "onnx:     $ort (version $ver)"
} else {
    Note "onnx:     onnxruntime.dll not found (AI runs fail; scripts\get-ai.bat installs it into $modelsDir)"
}
if (Test-Path $modelsDir) {
    $models = @(Get-ChildItem $modelsDir -Filter *.onnx -File -ErrorAction SilentlyContinue)
    $gb = [math]::Round((($models | Measure-Object Length -Sum).Sum) / 1GB, 2)
    Note "models:   $($models.Count) .onnx files, $gb GB in $modelsDir"
} else {
    Note "models:   no models folder ($modelsDir)"
}
foreach ($line in $summary) { if ($line -and $line -notlike 'check started*') { Say $line 'DarkGray' } }

# ---- Steps -----------------------------------------------------------------
# Lines worth reading in the summary. Errors and warnings come with the
# `-->` line that says where they are.
$important = @(
    '^(error|warning)(\[[A-Za-z0-9_]+\])?:',
    '^\s+--> ',
    'panicked at',
    '^test .+ \.\.\. FAILED$',
    '^---- .+ stdout ----$',
    '^test result: ',
    '^Diff in ',
    '^error: could not compile',
    '^\s+Finished '
)
$pattern = ($important | ForEach-Object { "($_)" }) -join '|'
$failures = 0

function Run-Step([string]$name, [string[]]$cargoArgs) {
    if ($Step -and $Step -ne $name) { return }
    $cmd = "cargo $($cargoArgs -join ' ')"
    Say ''
    Say "===== $name : $cmd =====" 'Cyan'
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $picked = [System.Collections.Generic.List[string]]::new()
    $warnings = 0
    & cargo @cargoArgs 2>&1 | ForEach-Object {
        # cargo writes to stderr, which PowerShell wraps as error records; an
        # empty stderr line would otherwise print as "...RemoteException".
        $text = if ($_ -is [System.Management.Automation.ErrorRecord]) { "$($_.Exception.Message)" } else { "$_" }
        if ($text -eq 'System.Management.Automation.RemoteException') { $text = '' }
        $color = if ($text -match '^error') { 'Red' } elseif ($text -match '^warning') { 'Yellow' } elseif ($text -match 'FAILED|panicked') { 'Red' } else { '' }
        Say $text $color
        if ($text -match '^warning(\[[A-Za-z0-9_]+\])?:' -and $text -notmatch 'generated \d+ warning') { $warnings++ }
        if ($text -match $pattern) { $picked.Add($text) }
    }
    $code = $LASTEXITCODE
    $watch.Stop()
    $secs = [math]::Round($watch.Elapsed.TotalSeconds)
    $status = if ($code -eq 0) { 'ok' } else { "FAILED (exit $code)"; $script:failures++ }
    $color = if ($code -eq 0) { 'Green' } else { 'Red' }
    Say "----- $name : $status in $secs s" $color
    Note ''
    Note "== $name : $status in $secs s : $cmd"
    if ($warnings -gt 0) { Note "   $warnings warning(s)" }
    $max = 80
    $shown = $picked | Select-Object -First $max
    foreach ($l in $shown) { Note "   $l" }
    if ($picked.Count -gt $max) { Note "   ... $($picked.Count - $max) more lines in target\check.log" }
}

Run-Step 'fmt' @('fmt', '--all', '--check')
Run-Step 'clippy' @('clippy', '--workspace', '--all-targets', '--', '-D', 'warnings')
Run-Step 'test' @('test', '--workspace', '--no-fail-fast')
if (-not $Quick) { Run-Step 'build' @('build', '--release', '-p', 'app', '-p', 'cli') }

$elapsed = [math]::Round(((Get-Date) - $started).TotalMinutes, 1)
$verdict = if ($failures -eq 0) { 'ALL OK' } else { "$failures step(s) FAILED" }
Note ''
Note "check finished $(Get-Date) : $verdict in $elapsed min"
$summary | Set-Content -Path $sumPath -Encoding UTF8
$log.Close()

Write-Host ''
Write-Host '================ summary ================' -ForegroundColor Cyan
Get-Content $sumPath | ForEach-Object { Write-Host $_ }
Write-Host ''
Write-Host "Full output: $logPath"
Write-Host "Summary:     $sumPath"
exit $failures
