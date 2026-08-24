@echo off
if "%~1"=="" (
  if exist "%~dp0BackupRestore.exe" (
    "%~dp0BackupRestore.exe"
  ) else (
    echo BackupRestore.exe is missing. Build the Rust GUI package first.
    exit /b 2
  )
) else (
  powershell.exe -WindowStyle Hidden -NoProfile -ExecutionPolicy Bypass -File "%~dp0BackupRestore.ps1" %*
)
