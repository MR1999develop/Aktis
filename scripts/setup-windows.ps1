# Installs what building Aktis needs on a new Windows PC, with winget:
#   - Git,
#   - Visual Studio 2022 Build Tools with the C++ workload (MSVC linker and
#     Windows SDK, needed by Rust on Windows),
#   - rustup, then the toolchain pinned in rust-toolchain.toml with rustfmt
#     and clippy.
# Already installed tools are left alone. Each installer may ask for
# administrator rights (UAC). The output goes to target\setup.log too.
#
#   scripts\setup-windows.bat
$ErrorActionPreference = 'Continue'
Set-Location (Split-Path $PSScriptRoot)
New-Item -ItemType Directory -Force target | Out-Null
Start-Transcript -Path 'target\setup.log' -Force | Out-Null

$failures = 0

function Step([string]$text) { Write-Host ''; Write-Host "===== $text" -ForegroundColor Cyan }
function Fail([string]$text) { Write-Host $text -ForegroundColor Red; $script:failures++ }

# PATH as the system has it now, so tools installed by this script are found
# without opening a new window.
function Update-Path {
    $machine = [System.Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [System.Environment]::GetEnvironmentVariable('Path', 'User')
    $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    $env:Path = (@($machine, $user, $cargoBin) | Where-Object { $_ }) -join ';'
}

function Have([string]$exe) { [bool](Get-Command $exe -ErrorAction SilentlyContinue) }

function Winget-Install([string]$id, [string[]]$extra) {
    $common = @('install', '--id', $id, '--exact', '--source', 'winget',
        '--accept-package-agreements', '--accept-source-agreements')
    & winget @common @extra
    # winget returns non-zero when the package is already there; what counts
    # is whether the tool is found afterwards.
    Write-Host "winget exit code $LASTEXITCODE"
}

# Visual Studio installs that have the MSVC compiler.
function Have-Msvc {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) { return $false }
    $found = & $vswhere -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    return [bool]$found
}

Step 'winget'
if (-not (Have 'winget')) {
    Fail 'winget not found. Install "App Installer" from the Microsoft Store (or update it), then run this script again.'
    Stop-Transcript | Out-Null
    exit 1
}
& winget --version

Step 'Git'
if (Have 'git') { Write-Host 'already installed' } else {
    Winget-Install 'Git.Git' @('--silent')
    Update-Path
}
if (Have 'git') { & git --version } else { Fail 'Git is not installed.' }

Step 'Visual Studio 2022 Build Tools (C++)'
if (Have-Msvc) { Write-Host 'already installed' } else {
    Write-Host 'This takes a while (several GB); the Visual Studio installer shows its progress.'
    Winget-Install 'Microsoft.VisualStudio.2022.BuildTools' @('--override',
        '--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended')
}
if (Have-Msvc) { Write-Host 'MSVC found' } else { Fail 'MSVC build tools are not installed.' }

Step 'rustup'
Update-Path
if (Have 'rustup') { Write-Host 'already installed' } else {
    Winget-Install 'Rustlang.Rustup' @()
    Update-Path
}
if (Have 'rustup') { & rustup --version } else { Fail 'rustup is not installed.' }

Step 'Rust toolchain from rust-toolchain.toml'
if (Have 'rustup') {
    $toml = Get-Content 'rust-toolchain.toml' -Raw
    $channel = if ($toml -match 'channel\s*=\s*"([^"]+)"') { $Matches[1] } else { 'stable' }
    & rustup toolchain install $channel --profile minimal --component rustfmt --component clippy
    if ($LASTEXITCODE -ne 0) { Fail "rustup could not install $channel." }
    & rustc -V
    & cargo -V
}

Step 'Git settings for this folder'
if (Have 'git') {
    # The folder was copied from another PC: without this git refuses to work
    # in it ("dubious ownership").
    $here = (Get-Location).Path -replace '\\', '/'
    $safe = @(& git config --global --get-all safe.directory 2>$null)
    if ($safe -notcontains $here) { & git config --global --add safe.directory $here }
    Write-Host "safe.directory: $here"
    $name = & git config --global user.name
    $mail = & git config --global user.email
    if (-not $name) {
        $name = Read-Host 'Name for your git commits'
        if ($name) { & git config --global user.name $name }
    }
    if (-not $mail) {
        $mail = Read-Host 'E-mail for your git commits'
        if ($mail) { & git config --global user.email $mail }
    }
    Write-Host "user: $(& git config --global user.name) <$(& git config --global user.email)>"
    & git status --short --branch | Select-Object -First 5
}

Write-Host ''
if ($failures -eq 0) {
    Write-Host 'Setup finished: ALL OK. Next: scripts\check.bat (the first build downloads the crates and takes several minutes).' -ForegroundColor Green
} else {
    Write-Host "Setup finished with $failures problem(s): see the messages above (also in target\setup.log)." -ForegroundColor Red
}
Stop-Transcript | Out-Null
exit $failures
