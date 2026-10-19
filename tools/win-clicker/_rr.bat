@echo off
setlocal
rem PE 内手动 recover 通道验收批处理（2026-10-19）。
rem 探到 workspace 卷（含 brwork 目录）后，不带 --efi-root 跑 recover：
rem   D2 镜像卷按卷 GUID 重解析盘符；D3 STEP 1 前自动挂 ESP；D4 独立恢复分区重接。
set ROOT=
for %%L in (C D E F G H I J K L M N O P Q R S T U V W Y Z) do (
  if exist "%%L:\brwork\BackupRestore.exe" set ROOT=%%L:\brwork
)
if "%ROOT%"=="" (
  echo ROOT_NOT_FOUND
  goto :end
)
echo ROOT=%ROOT%
echo TASK_ID=a4c44b48-be03-4f40-ae4c-dbdd5b56472c
"%ROOT%\BackupRestore.exe" recover %ROOT% a4c44b48-be03-4f40-ae4c-dbdd5b56472c > "%ROOT%\pe-recover.log" 2>&1
echo EXIT=%errorlevel% >> "%ROOT%\pe-recover.log"
echo DONE_EXIT=%errorlevel%
:end
