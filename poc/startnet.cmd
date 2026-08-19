@echo off
setlocal

rem WinRE PoC entry point. Find the persistent Windows volume.
set "WINVOL="
for %%D in (C D E F G H I J K L) do if exist "%%D:\Windows\System32\winload.efi" set "WINVOL=%%D:"
if not defined WINVOL for %%D in (C D E F G H I J K L) do if exist "%%D:\Windows\System32" set "WINVOL=%%D:"
if not defined WINVOL set "WINVOL=C:"
set "TASK_ROOT=%WINVOL%\WinRE-PoC"
if not exist "%TASK_ROOT%" mkdir "%TASK_ROOT%"
>"%TASK_ROOT%\startup-seen.txt" echo startnet.cmd started

rem RecoveryPoC.cmd is copied beside this script in the staged WinRE image.
if exist "%~dp0RecoveryPoC.cmd" (
    call "%~dp0RecoveryPoC.cmd" "%WINVOL%"
) else (
    >"%TASK_ROOT%\startup-error.txt" echo RecoveryPoC.cmd missing
)

if exist "%TASK_ROOT%\original\Winre.wim" (
    for %%D in (C D E F G H I J K L) do if exist "%%D:\Recovery\WindowsRE\Winre.wim" (
        copy /y "%TASK_ROOT%\original\Winre.wim" "%%D:\Recovery\WindowsRE\Winre.wim" >nul
        >"%TASK_ROOT%\restore-seen.txt" echo original WinRE restored
    )
)

wpeutil reboot
endlocal
