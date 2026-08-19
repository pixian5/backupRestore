@echo off
setlocal

rem Minimal WinRE proof: leave a persistent marker and do no disk operations.
set "WINVOL=%~1"
if not defined WINVOL set "WINVOL=C:"
set "MARKER_DIR=%WINVOL%\WinRE-PoC"
if not exist "%MARKER_DIR%" mkdir "%MARKER_DIR%"
>"%MARKER_DIR%\recovery-started.txt" echo RecoveryPoC.cmd started
>>"%MARKER_DIR%\recovery-started.txt" echo source=WinRE-startnet

endlocal
