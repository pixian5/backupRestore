@echo off
setlocal EnableExtensions DisableDelayedExpansion
set "LOG=C:\WinRE-PoC\mountvol-direct.txt"
set "VOLUME=\\?\Volume{761230e8-107c-4396-8c37-82273720183d}\"
set "MOUNT=C:\WinRE-PoC\mount-test"

>"%LOG%" echo volume=%VOLUME%
if exist "%MOUNT%" rmdir "%MOUNT%"
mkdir "%MOUNT%"
mountvol "%MOUNT%" %VOLUME% >>"%LOG%" 2>&1
>>"%LOG%" echo mount_exit=%errorlevel%
if exist "%MOUNT%\Windows" (
  >>"%LOG%" echo mounted=yes
  dir "%MOUNT%\" >>"%LOG%" 2>&1
) else (
  >>"%LOG%" echo mounted=no
)
mountvol "%MOUNT%" /D >>"%LOG%" 2>&1
>>"%LOG%" echo unmount_exit=%errorlevel%
rmdir "%MOUNT%"
