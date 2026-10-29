@echo off
rem Installs Git, the Visual Studio C++ build tools, rustup and the pinned Rust
rem toolchain on a new Windows PC (see scripts\setup-windows.ps1). Log in
rem target\setup.log.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0setup-windows.ps1" %*
pause
