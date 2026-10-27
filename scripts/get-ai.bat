@echo off
rem Downloads what the AI tools use (ONNX Runtime and the models) ahead of time.
rem See scripts\get-ai.ps1. Optional: the editor offers each download when a
rem tool first needs it. Arguments: a tool name (denoise, subject, objects,
rem people, parts, tagging); none means everything.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0get-ai.ps1" %*
pause
