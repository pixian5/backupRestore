@echo off
mkdir E:\backupRestore-package 2>nul
copy /y C:\Users\Public\backupRestore-package\BackupRestore.exe E:\backupRestore-package\ >nul
copy /y C:\Users\Public\backupRestore-package\Recovery.exe E:\backupRestore-package\ >nul
copy /y C:\Users\Public\backupRestore-package\RecoveryLauncher.cmd E:\backupRestore-package\ >nul
copy /y C:\Users\Public\backupRestore-package\winpeshl.ini E:\backupRestore-package\ >nul
copy /y C:\Users\Public\backupRestore-package\VCRUNTIME140.dll E:\backupRestore-package\ >nul
copy /y C:\Users\Public\backupRestore-package\VCRUNTIME140_1.dll E:\backupRestore-package\ >nul
echo COPIED > C:\Users\Public\copy_to_e.txt
dir /b E:\backupRestore-package\ >> C:\Users\Public\copy_to_e.txt
