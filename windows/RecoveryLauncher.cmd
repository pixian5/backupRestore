@echo off
setlocal EnableExtensions EnableDelayedExpansion

set "SYSTEM32=%~dp0"
set "CONFIG=%SYSTEM32%RecoveryTask.env"
set "EARLY_LOG=C:\WinRE-PoC\Recovery-launcher.log"
if not exist "C:\WinRE-PoC" mkdir "C:\WinRE-PoC"
>"%EARLY_LOG%" echo RecoveryLauncher.cmd entered
if not exist "%CONFIG%" (
  >>"%EARLY_LOG%" echo RecoveryTask.env missing
  exit /b 87
)
for /f "usebackq tokens=1,* delims==" %%A in ("%CONFIG%") do set "%%A=%%B"
call :verify_file_sha "%SYSTEM32%RecoveryLauncher.cmd" "%EXPECTED_LAUNCHER_SHA256%"
if errorlevel 1 (
  >>"%EARLY_LOG%" echo launcher hash mismatch
  exit /b 22
)

if exist "%SYSTEM32%Recovery.exe" (
  call :verify_file_sha "%SYSTEM32%Recovery.exe" "%EXPECTED_RECOVERY_SHA256%"
  if errorlevel 1 (
    >>"%EARLY_LOG%" echo Recovery.exe hash mismatch
    exit /b 23
  )
  >>"%EARLY_LOG%" echo starting Recovery.exe
  "%SYSTEM32%Recovery.exe" recover-env "%CONFIG%"
  exit /b !errorlevel!
)

if /I "%OPERATION%"=="probe" (
  >>"%EARLY_LOG%" echo Recovery.exe absent; using probe compatibility script
  call "%SYSTEM32%Recovery.cmd"
  exit /b !errorlevel!
)
>>"%EARLY_LOG%" echo Recovery.exe missing for a real operation
exit /b 24

:verify_file_sha
if not exist "%~1" exit /b 1
if "%~2"=="" exit /b 1
set "VERIFY_HASH="
for /f "skip=1 tokens=1" %%H in ('certutil.exe -hashfile "%~1" SHA256 2^>nul') do if not defined VERIFY_HASH set "VERIFY_HASH=%%H"
if /I not "!VERIFY_HASH!"=="%~2" exit /b 1
exit /b 0
