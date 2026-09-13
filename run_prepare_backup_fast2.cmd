@echo off
cd /d C:\Users\Public\backupRestore-package
BackupRestore.exe prepare --operation backup --source-drive C --target-drive C --boot-menu-name "BackupRestore Test" --image-path E:\fast2.wim --wim-index 1 --compress fast > C:\Users\Public\prepare_fast2_out.txt 2>&1
