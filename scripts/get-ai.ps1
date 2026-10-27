# Downloads what AI selection needs on Windows, into the models folder
# (%LOCALAPPDATA%\raw-node-editor\models, or $env:RNE_MODELS_DIR):
#   - ONNX Runtime with DirectML (any GPU: NVIDIA, AMD, Intel) and DirectML.dll,
#   - the AI models listed by `rne models`, then checks their SHA-256.
# Files already present are not downloaded again. About 2 GB in total.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is much faster without the progress bar
Set-Location (Split-Path $PSScriptRoot)

$models = if ($env:RNE_MODELS_DIR) { $env:RNE_MODELS_DIR } else { Join-Path $env:LOCALAPPDATA 'raw-node-editor\models' }
New-Item -ItemType Directory -Force $models | Out-Null
Write-Host "Models folder: $models"

function Get-NuGet($id, $version, $inner, $name) {
    $target = Join-Path $models $name
    if (Test-Path $target) { Write-Host "  $name already there"; return }
    $zip = Join-Path $env:TEMP "$id.$version.zip"
    $out = Join-Path $env:TEMP "$id.$version"
    Write-Host "  downloading $id $version"
    Invoke-WebRequest "https://www.nuget.org/api/v2/package/$id/$version" -OutFile $zip
    if (Test-Path $out) { Remove-Item -Recurse -Force $out }
    Expand-Archive $zip $out
    Copy-Item (Join-Path $out $inner) $target
}

Write-Host 'ONNX Runtime (DirectML):'
Get-NuGet 'Microsoft.ML.OnnxRuntime.DirectML' '1.24.4' 'runtimes\win-x64\native\onnxruntime.dll' 'onnxruntime.dll'
Get-NuGet 'Microsoft.AI.DirectML' '1.15.4' 'bin\x64-win\DirectML.dll' 'DirectML.dll'

Write-Host 'Building rne...'
cargo build --release -q -p cli
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$rne = 'target\release\rne.exe'

Write-Host 'Models:'
$env:RNE_MODELS_DIR = $models
$urls = & $rne models | Select-String -Pattern 'https://\S+\.onnx' -AllMatches | ForEach-Object { $_.Matches.Value }
foreach ($url in $urls) {
    $file = Join-Path $models ([System.IO.Path]::GetFileName($url))
    Write-Host "  downloading $([System.IO.Path]::GetFileName($url))"
    Invoke-WebRequest $url -OutFile "$file.part"
    Move-Item -Force "$file.part" $file
}
& $rne models --verify
