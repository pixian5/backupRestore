@echo off
rem Dry run: drive the GUI up to just before the confirm dialog. No reboot.
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\pkg\re-gui-run.ps1 -StopBefore 1 > C:\Users\Public\pkg\re-gui-dry.out 2>&1
echo EXIT=%ERRORLEVEL% >> C:\Users\Public\pkg\re-gui-dry.out
