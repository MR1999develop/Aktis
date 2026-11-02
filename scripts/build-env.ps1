# Takes the paths of this computer out of what cargo builds: the user's
# folder, Cargo's folder and the project folder are written into the
# programs (panic messages, debug data) as short names instead
# (--remap-path-prefix), so no executable carries them.
#
# Dot-sourced by every script that builds:  . "$PSScriptRoot\build-env.ps1"
# The flags differ from a plain `cargo` run, so switching between the two
# rebuilds the workspace once.

$project = Split-Path $PSScriptRoot
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
# The last matching prefix wins, so the most specific folder goes last.
$remaps = @(
    "--remap-path-prefix=$env:USERPROFILE=~",
    "--remap-path-prefix=$cargoHome=cargo",
    "--remap-path-prefix=$project=aktis"
)
# One flag per field, separated by 0x1F: paths may contain spaces.
$env:CARGO_ENCODED_RUSTFLAGS = $remaps -join [char]0x1F
