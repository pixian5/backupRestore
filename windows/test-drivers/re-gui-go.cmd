@echo off
rem Full run: GUI -> offline environment dialog -> Windows RE -> confirm Yes -> prepare -> real reboot.
powershell -NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\pkg\re-gui-run.ps1 > C:\Users\Public\pkg\re-gui-go.out 2>&1
echo EXIT=%ERRORLEVEL% >> C:\Users\Public\pkg\re-gui-go.out
