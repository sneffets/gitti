@echo off
rem Runs install-windows.ps1 regardless of the PowerShell execution policy.
rem   scripts\install-windows.cmd
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-windows.ps1" %*
exit /b %errorlevel%
