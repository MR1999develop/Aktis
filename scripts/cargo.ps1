# Runs cargo with the arguments given, without the paths of this computer in
# what it builds (see build-env.ps1). For the .bat scripts:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\cargo.ps1 build --release -p app
. "$PSScriptRoot\build-env.ps1"
& cargo @args
exit $LASTEXITCODE
