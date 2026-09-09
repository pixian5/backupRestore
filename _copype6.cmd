@echo off
if exist C:\WinPE_arm64 rmdir /s /q C:\WinPE_arm64
set WinPERoot=C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Windows Preinstallation Environment
set OSCDImgRoot=C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\arm64\Oscdimg
set DISMRoot=C:\Windows\System32
cd /d "%WinPERoot%"
call copype.cmd arm64 C:\WinPE_arm64 > C:\Users\x\Desktop\BackupRestore\_copype6.log 2>&1
echo EXIT=%errorlevel% >> C:\Users\x\Desktop\BackupRestore\_copype6.log
