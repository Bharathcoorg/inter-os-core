@echo off
title Inter OS — Smartphone Device Emulator Studio
echo ========================================================
echo   Launching Inter OS Smartphone Device Emulator Studio...
echo ========================================================
echo.
cd /d "%~dp0emulator"
python server.py
pause
