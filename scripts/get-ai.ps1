# Downloads everything the AI tools use, ahead of time, into the models folder
# (%LOCALAPPDATA%\aktis\models, or $env:AKTIS_MODELS_DIR):
#   - ONNX Runtime with DirectML (any GPU: NVIDIA, AMD, Intel) and DirectML.dll,
#   - SCUNet for AI denoise (and its faster large-tile variant),
#   - BiRefNet, Segment Anything 2.1, YOLOX and MediaPipe for AI selection,
#   - SigLIP 2 for AI keywords.
# The list is the program's own (`aktis-cli models`): the editor offers the same
# downloads the first time a tool needs them, so this script is optional.
# Files already installed are not downloaded again; each is checked.
#
#   scripts\get-ai.bat               everything (several GB)
#   scripts\get-ai.bat denoise       only what one tool needs
#                                    (denoise, subject, objects, people, parts, tagging)
$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot)
. "$PSScriptRoot\build-env.ps1"   # no local paths in the programs built

# @(...) keeps a list even with one name (PowerShell would unwrap it into a
# string, and splatting a string passes it one letter at a time).
$what = @(if ($args.Count -gt 0) { $args } else { 'all' })

Write-Host 'Building aktis-cli...'
cargo build --release -q -p cli
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
$cli = 'target\release\aktis-cli.exe'

& $cli models --download @what --yes
$failed = $LASTEXITCODE
Write-Host ''
& $cli models
if ($failed -ne 0) { Write-Host ''; Write-Host 'Some files were not installed: see the messages above.' -ForegroundColor Red }
exit $failed
