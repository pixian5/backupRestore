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

if exist "%SYSTEM32%Recovery.exe" (
  >>"%EARLY_LOG%" echo Recovery.exe present; payload hashes delegated to Rust
  >>"%EARLY_LOG%" echo starting Recovery.exe
  REM 捕获退出码 + 重定向 stderr，供判断：加载失败(0xC0000135)/执行报错(exit 1)/
  REM 成功但未进 recover_env(exit 0 且无 C:\BackupRestore-Recovery-early.log)。
  "%SYSTEM32%Recovery.exe" recover-env "%CONFIG%" 1>>"%EARLY_LOG%" 2>&1
  set "REC_EXIT=!errorlevel!"
  >>"%EARLY_LOG%" echo RECOVERY_EXIT_CODE=!REC_EXIT!
  echo RECOVERY_EXIT_CODE=!REC_EXIT!
  set "EXITOK=1"
  if "!REC_EXIT!"=="0" set "EXITOK=0"
  if !EXITOK!==1 (
    >>"%EARLY_LOG%" echo RECOVERY_FAILED_WITH_CODE=!REC_EXIT!
  )
  exit /b !REC_EXIT!
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
set "EXPECTED_HASH=%~2"
for /f "skip=1 tokens=1" %%H in ('certutil.exe -hashfile "%~1" SHA256 2^>nul') do if not defined VERIFY_HASH set "VERIFY_HASH=%%H"
set "VERIFY_HASH=!VERIFY_HASH: =!"
set "EXPECTED_HASH=!EXPECTED_HASH: =!"
if /I not "!VERIFY_HASH!"=="!EXPECTED_HASH!" (
  >>"%EARLY_LOG%" echo hash mismatch file=%~1 expected=!EXPECTED_HASH! actual=!VERIFY_HASH!
  exit /b 1
)
exit /b 0
