@echo off
setlocal EnableExtensions EnableDelayedExpansion

set "SYSTEM32=%~dp0"
set "CONFIG=%SYSTEM32%RecoveryTask.env"
rem Launcher diagnostics live in the ephemeral WinRE payload. Recovery.exe
rem switches all persistent logging to the mounted program directory.
set "EARLY_LOG=%SYSTEM32%Recovery-launcher-early.log"
>"%EARLY_LOG%" echo RecoveryLauncher.cmd entered
if not exist "%CONFIG%" (
  >>"%EARLY_LOG%" echo RecoveryTask.env missing
  exit /b 87
)
for /f "usebackq tokens=1,* delims==" %%A in ("%CONFIG%") do set "%%A=%%B"

if not exist "%SYSTEM32%Recovery.exe" (
  >>"%EARLY_LOG%" echo Recovery.exe missing
  exit /b 24
)
>>"%EARLY_LOG%" echo starting Rust Recovery.exe
"%SYSTEM32%Recovery.exe" recover-env "%CONFIG%"
exit /b !errorlevel!
