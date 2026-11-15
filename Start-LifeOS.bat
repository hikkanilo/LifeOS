@echo off
setlocal
set "APP=%~dp0release\LifeOS.exe"
if not exist "%APP%" (
  echo LifeOS is not built yet. Run Setup-LifeOS.ps1 first.
  pause
  exit /b 1
)
start "" "%APP%"
