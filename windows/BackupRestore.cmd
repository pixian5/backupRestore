@echo off
if "%~1"=="" (
  if exist "%~dp0BackupRestore.exe" (
    "%~dp0BackupRestore.exe"
  ) else (
    powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0BackupRestore.Gui.ps1"
  )
) else (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0BackupRestore.ps1" %*
)
