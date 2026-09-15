@echo off
setlocal EnableExtensions
set "LOG=C:\WinRE-PoC\relocate2.log"
set "REC=C:\Recovery\WindowsRE"
if exist "%LOG%" del "%LOG%" >nul 2>&1
goto :main

:run
echo [%1] %2 %3 %4 %5 >> "%LOG%"
shift
%2 %3 %4 %5 %6 %7 %8 >> "%LOG%" 2>&1
echo   exitcode=%errorlevel% >> "%LOG%"
goto :eof
:main

mkdir "%REC%" 2>nul
echo === copy files === >> "%LOG%"
copy /b /y "Y:\Recovery\WindowsRE\Winre.wim" "%REC%\Winre.wim" >> "%LOG%" 2>&1
echo   copy wim exitcode=%errorlevel% >> "%LOG%"
copy /b /y "Y:\Recovery\WindowsRE\boot.sdi" "%REC%\boot.sdi" >> "%LOG%" 2>&1
echo   copy sdi exitcode=%errorlevel% >> "%LOG%"

echo === ramdiskoptions {e7e9964b} === >> "%LOG%"
call :run ramdisksdidevice bcdedit /set {e7e9964b} ramdisksdidevice partition=C:
call :run ramdisksdipath bcdedit /set {e7e9964b} ramdisksdipath \Recovery\WindowsRE\boot.sdi

echo === winre loader {e7e9964a} === >> "%LOG%"
call :run dev bcdedit /set {e7e9964a} device ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{e7e9964b}
call :run osdev bcdedit /set {e7e9964a} osdevice ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{e7e9964b}

echo === bootmgr === >> "%LOG%"
call :run def bcdedit /set {9dea862c} default {e7e9964a}
call :run bseq bcdedit /set {9dea862c} bootsequence {e7e9964a}
call :run to bcdedit /timeout 0

echo === verify === >> "%LOG%"
dir "%REC%" >> "%LOG%" 2>&1
bcdedit /enum {e7e9964a} {e7e9964b} /v >> "%LOG%" 2>&1
bcdedit /enum bootmgr /v >> "%LOG%" 2>&1
echo === RELOCATE2 DONE === >> "%LOG%"
echo DONE