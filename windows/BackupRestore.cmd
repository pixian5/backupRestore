@echo off
if not exist "%~dp0BackupRestore.exe" exit /b 2
"%~dp0BackupRestore.exe" %*
