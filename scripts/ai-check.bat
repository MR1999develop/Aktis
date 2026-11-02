@echo off
rem Tries AI selection on a photo: drag a RAW file (or an 8-bit .ppm) onto this
rem script. The previews (selection in red) go to target\ai-check\ and the
rem log to target\ai-check\log.txt. Needs scripts\get-ai.bat first.
setlocal
if "%~1"=="" (echo Drag a RAW file onto this script. & pause & exit /b 1)
set PHOTO=%~f1
cd /d "%~dp0.."
set OUT=target\ai-check
if not exist %OUT% mkdir %OUT%
set LOG=%OUT%\log.txt
echo ai-check %date% %time% %PHOTO% > "%LOG%"
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0cargo.ps1" build --release -q -p cli >> "%LOG%" 2>&1
set CLI=target\release\aktis-cli.exe
%CLI% models >> "%LOG%" 2>&1
call :run subject  --subject
call :run people   --people
call :run parts    --parts hair,face-skin,body-skin,clothes,eyebrows,eye-sclera,irises,lips,teeth
call :run no-faces --people --subtract --parts face-skin
call :run brush    --subject --brush 0.03:0.1,0.1:0.3,0.15 --erase 0.05:0.5,0.5
echo.
type "%LOG%"
echo.
echo Previews in %OUT%
pause
exit /b 0

:run
set NAME=%1
shift
echo. >> "%LOG%"
echo ===== %NAME% ===== >> "%LOG%"
%CLI% mask "%PHOTO%" %1 %2 %3 %4 %5 %6 %7 %8 %9 -o %OUT%\%NAME%.jpg >> "%LOG%" 2>&1
exit /b 0
