@echo off
rem Starts the editor (release build). Drag a RAW file onto this script to open it.
cd /d "%~dp0.."
cargo run --release -p app -- %*
if errorlevel 1 pause
