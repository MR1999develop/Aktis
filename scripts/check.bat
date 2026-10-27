@echo off
rem Local checks: format, clippy, tests and release build, with everything on
rem screen. Full output in target\check.log, a summary with the errors of
rem each step in target\check-summary.txt (read by Claude to fix them).
rem Options: -Quick (no release build), -Step fmt^|clippy^|test^|build.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0check.ps1" %*
pause
