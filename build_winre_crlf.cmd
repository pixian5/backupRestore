@echo off
setlocal
set "OUT=C:\Users\Public\winre_build_out.txt"
echo === START %DATE% %TIME% === > %OUT%

rem 1. copype arm64 构建 WinPE 工作目录
call "C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Windows Preinstallation Environment\copype.cmd" arm64 C:\winre-build >> %OUT% 2>&1
echo COPEXIT=%ERRORLEVEL% >> %OUT%

rem 2. 准备 WinRE 目录，把 boot.wim 复制为 Winre.wim
if exist C:\winre-build\media\sources\boot.wim (
    mkdir C:\WinRE 2>nul
    copy /y C:\winre-build\media\sources\boot.wim C:\WinRE\Winre.wim >> %OUT% 2>&1
    echo COPYEXIT=%ERRORLEVEL% >> %OUT%
) else (
    echo BOOTWIM-MISSING >> %OUT%
)

rem 3. 注册并启用 WinRE
reagentc /setreimage /path C:\WinRE >> %OUT% 2>&1
echo SETREXIT=%ERRORLEVEL% >> %OUT%
reagentc /enable >> %OUT% 2>&1
echo ENABLEEXIT=%ERRORLEVEL% >> %OUT%
reagentc /info >> %OUT% 2>&1
echo === DONE %DATE% %TIME% === >> %OUT%
endlocal
