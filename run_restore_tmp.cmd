@echo off
cd /d C:\Users\Public\backupRestore-package
echo === RUN RESTORE prepare at %DATE% %TIME% === > C:\Users\Public\restore_out.txt
BackupRestore.exe prepare --operation restore --source-drive C --target-drive C --image-path E:\1.wim --wim-index 1 --allow-destructive >> C:\Users\Public\restore_out.txt 2>&1
echo EXIT_CODE=%ERRORLEVEL% >> C:\Users\Public\restore_out.txt
