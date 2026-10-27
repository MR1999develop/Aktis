@echo off
rem Downloads ONNX Runtime (DirectML) and the AI models for AI selection.
rem See scripts\get-ai.ps1. Run once; run again after a model update.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0get-ai.ps1"
pause
