@echo off
setlocal EnableExtensions EnableDelayedExpansion

if /I "%~1"=="--self-test" goto self_test

set "SYSTEM32=%~dp0"
set "CONFIG=%SYSTEM32%RecoveryTask.env"
set "EARLY_LOG=C:\WinRE-PoC\Recovery-early.log"
if not exist "C:\WinRE-PoC" mkdir "C:\WinRE-PoC"
>"%EARLY_LOG%" echo Recovery.cmd entered
if not exist "%CONFIG%" goto early_config_missing
for /f "usebackq tokens=1,* delims==" %%A in ("%CONFIG%") do set "%%A=%%B"
>>"%EARLY_LOG%" echo configuration loaded
call :verify_file_sha "%SYSTEM32%Recovery.cmd" "%EXPECTED_RECOVERY_CMD_SHA256%"
if errorlevel 1 goto early_payload_invalid
call :verify_file_sha "%SYSTEM32%task.json" "%EXPECTED_TASK_SHA256%"
if errorlevel 1 goto early_payload_invalid
call :validate_configuration
if errorlevel 1 goto early_payload_invalid

set "STATUS=%TASK_ROOT_REL%\status.env"
set "LOG=%TASK_ROOT_REL%\Recovery.log"

call :ensure_volume "%TASK_VOLUME_GUID%" "%TASK_DISK_NUMBER%" "%TASK_PARTITION_NUMBER%" T TASK_MOUNT
if errorlevel 1 goto early_task_mount_failed
>>"%EARLY_LOG%" echo task volume mounted at %TASK_MOUNT%
set "TASK_ROOT=%TASK_MOUNT%\%TASK_ROOT_REL%"
if not exist "%TASK_ROOT%\original\Winre.wim" goto early_task_missing
set "STATUS=%TASK_ROOT%\status.env"
set "STATUS_JSON=%TASK_ROOT%\status.json"
set "LOG=%TASK_ROOT%\Recovery.log"

call :log "Recovery host started operation=%OPERATION% task=%TASK_ID%"
call :status recovery-started 0

call :ensure_volume "%RECOVERY_VOLUME_GUID%" "%RECOVERY_DISK_NUMBER%" "%RECOVERY_PARTITION_NUMBER%" R RECOVERY_MOUNT
if errorlevel 1 goto fail
if not exist "%RECOVERY_MOUNT%\Recovery\WindowsRE\Winre.wim" goto fail
if /I "%OPERATION%"=="probe" goto probe

call :ensure_volume "%SOURCE_VOLUME_GUID%" "%SOURCE_DISK_NUMBER%" "%SOURCE_PARTITION_NUMBER%" S SOURCE_MOUNT
if errorlevel 1 goto fail
call :ensure_volume "%IMAGE_VOLUME_GUID%" "%IMAGE_DISK_NUMBER%" "%IMAGE_PARTITION_NUMBER%" I IMAGE_MOUNT
if errorlevel 1 goto fail
if /I not "%OPERATION%"=="backup" (
  call :verify_file_sha "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%" "%IMAGE_SHA256%"
  if errorlevel 1 goto fail
)
if /I "%OPERATION%"=="restore" call :mount_restore_volumes
if /I "%OPERATION%"=="restore-existing" call :mount_restore_volumes
if /I "%OPERATION%"=="create-secondary" call :mount_restore_volumes
if errorlevel 1 goto fail

call :status preflight 5
if /I "%OPERATION%"=="backup" goto backup
if /I "%OPERATION%"=="restore" goto restore
if /I "%OPERATION%"=="restore-existing" goto restore
if /I "%OPERATION%"=="create-secondary" goto restore
call :log "Unsupported operation: %OPERATION%"
goto fail

:probe
call :log "Probe volume mounts succeeded"
>"%TASK_ROOT%\probe-success.txt" echo Recovery.cmd automatically started in WinRE
call :status success 100
goto cleanup

:backup
if exist "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%.partial" del /q "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%.partial"
for %%I in ("%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%") do if not exist "%%~dpI" mkdir "%%~dpI"
call :status capturing 10
call :log "DISM Capture started"
dism.exe /Capture-Image /ImageFile:"%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%.partial" /CaptureDir:"%SOURCE_MOUNT%" /Name:"BackupRestore %TASK_ID%" /Compress:max /CheckIntegrity >>"%LOG%" 2>&1
if errorlevel 1 goto fail
dism.exe /Get-WimInfo /WimFile:"%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%.partial" >>"%LOG%" 2>&1
if errorlevel 1 goto fail
move /y "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%.partial" "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%" >>"%LOG%" 2>&1
if errorlevel 1 goto fail
call :write_metadata "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%"
call :status success 100
goto cleanup

:restore
if /I not "%ALLOW_DESTRUCTIVE%"=="YES" (
  call :log "Restore refused: destructive flag missing"
  goto fail
)
if not exist "%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%" goto fail
dism.exe /Get-WimInfo /WimFile:"%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%" >>"%LOG%" 2>&1
if errorlevel 1 goto fail
call :status target-erased 20
call :log "Formatting explicitly selected target volume"
format "%TARGET_MOUNT%" /FS:NTFS /Q /V:Windows /Y >>"%LOG%" 2>&1
if errorlevel 1 goto fail
call :status image-applied 45
dism.exe /Apply-Image /ImageFile:"%IMAGE_MOUNT%\%IMAGE_RELATIVE_PATH%" /Index:%WIM_INDEX% /ApplyDir:"%TARGET_MOUNT%" >>"%LOG%" 2>&1
if errorlevel 1 goto fail
call :status boot-repaired 85
if /I "%OPERATION%"=="create-secondary" (
  bcdboot.exe "%TARGET_MOUNT%\Windows" /s "%EFI_MOUNT%" /f UEFI /addlast >>"%LOG%" 2>&1
) else (
  bcdboot.exe "%TARGET_MOUNT%\Windows" /s "%EFI_MOUNT%" /f UEFI >>"%LOG%" 2>&1
)
if errorlevel 1 goto fail
call :status success 100
goto cleanup

:fail
set "FAIL_CODE=!errorlevel!"
call :status failed 0
call :log "Recovery host failed errorlevel=!FAIL_CODE!"
if /I "%OPERATION%"=="probe" goto probe_failed

goto cleanup

:probe_failed
if exist "%TASK_ROOT%" >"%TASK_ROOT%\probe-failed.txt" echo Recovery.cmd failed before probe completion
call :log "Probe failed; leaving the WinRE shell open for diagnosis"
exit /b 1

:early_config_missing
>>"%EARLY_LOG%" echo RecoveryTask.env missing
exit /b 87

:early_task_mount_failed
>>"%EARLY_LOG%" echo task mount failed errorlevel=%errorlevel%
exit /b 20

:early_task_missing
>>"%EARLY_LOG%" echo task root missing original WinRE
exit /b 21

:early_payload_invalid
>>"%EARLY_LOG%" echo recovery payload hash validation failed
exit /b 22

:ensure_volume
call :assign_partition "%~2" "%~3" "%~4"
if errorlevel 1 (
  set "%~5="
  exit /b 1
)
if not exist "%~4:\." (
  set "%~5="
  exit /b 1
)
set "ACTUAL_VOLUME="
for /f "delims=" %%V in ('mountvol %~4: /L 2^>nul') do set "ACTUAL_VOLUME=%%V"
if not defined ACTUAL_VOLUME (
  set "%~5="
  exit /b 1
)
if /I not "!ACTUAL_VOLUME!"=="%~1" (
  >>"%EARLY_LOG%" echo volume identity mismatch letter=%~4 expected=%~1 actual=!ACTUAL_VOLUME!
  set "%~5="
  exit /b 1
)
set "%~5=%~4:"
exit /b 0

:validate_configuration
rem The Rust host applies the same checks.  Keep the compatibility fallback
rem conservative because it may run without the Rust executable in WinRE.
if not defined TASK_ID exit /b 1
for /f "delims=0123456789abcdefABCDEF-" %%A in ("!TASK_ID!") do exit /b 1
if not defined TASK_ROOT_REL exit /b 1
if /I not "!TASK_ROOT_REL:~0,20!"=="BackupRestore\tasks\" exit /b 1
if /I not "!TASK_ROOT_REL!"=="BackupRestore\tasks\!TASK_ID!" exit /b 1
if not defined OPERATION exit /b 1
if /I not "!OPERATION!"=="probe" if /I not "!OPERATION!"=="backup" if /I not "!OPERATION!"=="restore" if /I not "!OPERATION!"=="restore-existing" if /I not "!OPERATION!"=="create-secondary" exit /b 1
if not defined IMAGE_RELATIVE_PATH if /I not "!OPERATION!"=="probe" exit /b 1
if defined IMAGE_RELATIVE_PATH (
  if "!IMAGE_RELATIVE_PATH:~0,1!"=="\" exit /b 1
  if "!IMAGE_RELATIVE_PATH:~0,1!"=="/" exit /b 1
  if "!IMAGE_RELATIVE_PATH:~0,1!"==":" exit /b 1
  echo(!IMAGE_RELATIVE_PATH!| findstr.exe /L /C:".." >nul && exit /b 1
  echo(!IMAGE_RELATIVE_PATH!| findstr.exe /L /C:":" >nul && exit /b 1
)
for %%G in (SOURCE_PARTITION_TYPE_GUID IMAGE_PARTITION_TYPE_GUID TARGET_PARTITION_TYPE_GUID) do (
  if defined %%G if /I "!%%G!"=="{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}" exit /b 1
  if defined %%G if /I "!%%G!"=="{e3c9e316-0b5c-4db8-817d-f92df00215ae}" exit /b 1
  if defined %%G if /I "!%%G!"=="{de94bba4-06d1-4d40-a16a-bfd50179d6ac}" exit /b 1
)
if /I not "!OPERATION!"=="probe" if /I "!IMAGE_VOLUME_GUID!"=="!TARGET_VOLUME_GUID!" exit /b 1
exit /b 0

:matches_volume
mountvol %~2: /L | findstr.exe /I /C:"%~1" >nul
exit /b %errorlevel%

:self_test
call :matches_volume "%~2" "%~3"
exit /b !errorlevel!

:assign_partition
set "DISKPART_SCRIPT=%SYSTEMROOT%\System32\BackupRestore-assign-%~3.txt"
>"%DISKPART_SCRIPT%" echo select disk %~1
>>"%DISKPART_SCRIPT%" echo select partition %~2
>>"%DISKPART_SCRIPT%" echo assign letter=%~3
diskpart.exe /s "%DISKPART_SCRIPT%" >>"%EARLY_LOG%" 2>&1
set "DISKPART_EXIT=!errorlevel!"
del /q "%DISKPART_SCRIPT%" >nul 2>&1
exit /b !DISKPART_EXIT!

:cleanup
call :log "Restoring original registered WinRE"
if not defined RECOVERY_MOUNT goto cleanup_failed
if not exist "%TASK_ROOT%\original\Winre.wim" goto cleanup_failed
copy /y "%TASK_ROOT%\original\Winre.wim" "%RECOVERY_MOUNT%\Recovery\WindowsRE\Winre.wim" >>"%LOG%" 2>&1
if errorlevel 1 goto cleanup_failed
call :verify_file_sha "%RECOVERY_MOUNT%\Recovery\WindowsRE\Winre.wim" "%ORIGINAL_WINRE_SHA256%"
if errorlevel 1 goto cleanup_failed
call :log "Rebooting"
wpeutil.exe reboot
exit /b 0

:cleanup_failed
call :log "Original WinRE was not restored; do not delete this task directory"
exit /b 1

:mount_restore_volumes
call :ensure_volume "%TARGET_VOLUME_GUID%" "%TARGET_DISK_NUMBER%" "%TARGET_PARTITION_NUMBER%" W TARGET_MOUNT
if errorlevel 1 exit /b 1
call :ensure_volume "%EFI_VOLUME_GUID%" "%EFI_DISK_NUMBER%" "%EFI_PARTITION_NUMBER%" E EFI_MOUNT
if errorlevel 1 exit /b 1
exit /b 0

:status
>"%STATUS%.tmp" echo stage=%~1
>>"%STATUS%.tmp" echo progress=%~2
>>"%STATUS%.tmp" echo task_id=%TASK_ID%
move /y "%STATUS%.tmp" "%STATUS%" >nul
if defined STATUS_JSON (
  >"%STATUS_JSON%.tmp" echo {"taskId":"%TASK_ID%","operation":"%OPERATION%","stage":"%~1","progress":%~2,"updated":"%TASK_CREATED%"}
  move /y "%STATUS_JSON%.tmp" "%STATUS_JSON%" >nul
)
exit /b 0

:write_metadata
set "META_HASH="
for /f "skip=1 tokens=1" %%H in ('certutil.exe -hashfile "%~1" SHA256 2^>nul') do if not defined META_HASH set "META_HASH=%%H"
if not defined META_HASH exit /b 1
for %%F in ("%~1") do set "META_SIZE=%%~zF"
if "%META_SIZE%"=="0" exit /b 1
set "META_PATH=%~dp1metadata.json"
>"%META_PATH%" echo {"version":1,"type":"wim","created":"%TASK_CREATED%","computer":"WinRE","windowsEdition":"%WINDOWS_EDITION%","architecture":"%WINDOWS_ARCHITECTURE%","windowsBuild":"%WINDOWS_BUILD%","wimIndex":%WIM_INDEX%,"imageSha256":"%META_HASH%","imageSize":%META_SIZE%,"source":{"diskGuid":"%SOURCE_DISK_GUID%","partitionGuid":"%SOURCE_PARTITION_GUID%","volumeGuid":"%SOURCE_VOLUME_GUID%","partitionTypeGuid":"%SOURCE_PARTITION_TYPE_GUID%","diskNumber":%SOURCE_DISK_NUMBER%,"partitionNumber":%SOURCE_PARTITION_NUMBER%,"partitionOffset":%SOURCE_PARTITION_OFFSET%,"partitionSize":%SOURCE_PARTITION_SIZE%,"filesystem":"%SOURCE_FILESYSTEM%","volumeSerial":"%SOURCE_VOLUME_SERIAL%"},"capturedUsedBytes":%SOURCE_USED_BYTES%,"reservedBytes":%RESERVED_BYTES%,"minimumTargetSize":%MINIMUM_TARGET_SIZE%,"volumeSerial":"%SOURCE_VOLUME_SERIAL%","programVersion":"%PROGRAM_VERSION%"}
exit /b 0

:verify_file_sha
if not exist "%~1" exit /b 1
if "%~2"=="" exit /b 1
set "VERIFY_HASH="
for /f "skip=1 tokens=1" %%H in ('certutil.exe -hashfile "%~1" SHA256 2^>nul') do if not defined VERIFY_HASH set "VERIFY_HASH=%%H"
if /I not "!VERIFY_HASH!"=="%~2" exit /b 1
exit /b 0

:log
>>"%LOG%" echo [%date% %time%] %~1
exit /b 0
