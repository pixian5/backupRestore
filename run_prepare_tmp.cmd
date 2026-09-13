@echo off
cd /d C:\Users\Public\backupRestore-package
echo === RUN prepare at %DATE% %TIME% === > C:\Users\Public\prepare_out.txt
BackupRestore.exe prepare --operation backup --source-drive C --target-drive C --boot-menu-name "BackupRestore Test" --image-path E:\1.wim --wim-index 1 >> C:\Users\Public\prepare_out.txt 2>&1
echo EXIT_CODE=%ERRORLEVEL% >> C:\Users\Public\prepare_out.txt
