@echo off
cd /d C:\Users\Public\backupRestore-package
BackupRestore.exe prepare --operation restore --source-drive C --target-drive C --boot-menu-name "BackupRestore Test" --image-path E:\fast2.wim --wim-index 1 --allow-destructive > C:\Users\Public\restore_reloc_out.txt 2>&1
