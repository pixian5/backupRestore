@echo off
cd /d E:\backupRestore-package
echo === RUN RESTORE prepare (from E:) at %DATE% %TIME% === > C:\Users\Public\restore_out.txt
E:\backupRestore-package\BackupRestore.exe prepare --operation restore --source-drive C --target-drive C --image-path E:\1.wim --wim-index 1 --allow-destructive >> C:\Users\Public\restore_out.txt 2>&1
echo EXIT_CODE=%ERRORLEVEL% >> C:\Users\Public\restore_out.txt
