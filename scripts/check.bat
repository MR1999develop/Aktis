@echo off
rem Formats check, lint, build and tests on this PC.
rem The full output goes to target\check.log, a one-line-per-step summary
rem to target\check-summary.txt (read by Claude to fix errors).
setlocal
cd /d "%~dp0.."
if not exist target mkdir target
set LOG=target\check.log
set SUM=target\check-summary.txt
echo check started %date% %time% > "%LOG%"
echo check started %date% %time% > "%SUM%"

call :step fmt    cargo fmt --all --check
call :step clippy cargo clippy --workspace --all-targets -- -D warnings
call :step test   cargo test --workspace
call :step build  cargo build --release -p app -p cli

echo check finished %date% %time% >> "%SUM%"
echo.
type "%SUM%"
echo.
echo Full output: %LOG%
pause
exit /b 0

:step
echo.
echo ===== %1 =====
echo ===== %1 ===== >> "%LOG%"
shift
set CMD=%1 %2 %3 %4 %5 %6 %7 %8 %9
%CMD% >> "%LOG%" 2>&1
if errorlevel 1 (echo %CMD% : FAILED >> "%SUM%") else (echo %CMD% : ok >> "%SUM%")
exit /b 0
