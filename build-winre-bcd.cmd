@echo off
set STORE=R:\EFI\Microsoft\Boot\BCD
bcdedit /store %STORE% /create /d "Windows Recovery" /device > C:\Windows\Temp\ramguid.txt
for /f "tokens=2 delims={}" %%g in (C:\Windows\Temp\ramguid.txt) do set RAMGUID=%%g
echo RAMGUID=%RAMGUID%
bcdedit /store %STORE% /set {%RAMGUID%} ramdisksdidevice partition=R:
bcdedit /store %STORE% /set {%RAMGUID%} ramdisksdipath \Recovery\WindowsRE\boot.sdi
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} device ramdisk=[R:]\Recovery\WindowsRE\Winre.wim,{%RAMGUID%}
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} osdevice ramdisk=[R:]\Recovery\WindowsRE\Winre.wim,{%RAMGUID%}
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} path \windows\system32\winload.efi
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} systemroot \Windows
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} winpe Yes
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} detecthal Yes
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} nx OptIn
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} bootmenupolicy Standard
bcdedit /store %STORE% /set {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} locale zh-CN
bcdedit /store %STORE% /default {e6aed4d4-a1a1-11f1-b837-bf40ef171f42}
bcdedit /store %STORE% /displayorder {e6aed4d4-a1a1-11f1-b837-bf40ef171f42} /addfirst
echo ===R-BCD-ENUM===
bcdedit /store %STORE% /enum all /v
