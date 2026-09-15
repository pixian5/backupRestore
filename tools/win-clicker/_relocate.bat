@echo off
setlocal EnableExtensions
set "LOG=C:\WinRE-PoC\relocate.log"
set "REC=C:\Recovery\WindowsRE"

> "%LOG%" echo === relocate start ===
mkdir "%REC%" 2>nul
echo copying Winre.wim >> "%LOG%"
copy /Y "Y:\Recovery\WindowsRE\Winre.wim" "%REC%\Winre.wim" >> "%LOG%" 2>&1
echo copying boot.sdi >> "%LOG%"
copy /Y "Y:\Recovery\WindowsRE\boot.sdi" "%REC%\boot.sdi" >> "%LOG%" 2>&1

echo === bcdedit ramdiskoptions {e7e9964b} === >> "%LOG%"
bcdedit /set {e7e9964b} ramdisksdidevice partition=C: >> "%LOG%" 2>&1
bcdedit /set {e7e9964b} ramdisksdipath \Recovery\WindowsRE\boot.sdi >> "%LOG%" 2>&1

echo === bcdedit winre loader {e7e9964a} === >> "%LOG%"
bcdedit /set {e7e9964a} device ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{e7e9964b} >> "%LOG%" 2>&1
bcdedit /set {e7e9964a} osdevice ramdisk=[C:]\Recovery\WindowsRE\Winre.wim,{e7e9964b} >> "%LOG%" 2>&1

echo === bootmgr default/bootsequence === >> "%LOG%"
bcdedit /set {9dea862c} default {e7e9964a} >> "%LOG%" 2>&1
bcdedit /set {9dea862c} bootsequence {e7e9964a} >> "%LOG%" 2>&1
bcdedit /timeout 0 >> "%LOG%" 2>&1

echo === verify listing === >> "%LOG%"
dir "%REC%" >> "%LOG%" 2>&1
bcdedit /enum {e7e9964a} {e7e9964b} /v >> "%LOG%" 2>&1
echo === RELOCATE DONE === >> "%LOG%"
echo DONE