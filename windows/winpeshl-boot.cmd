@echo off
setlocal EnableExtensions EnableDelayedExpansion
REM 纯探针：无论后面对 Recovery.exe 是否挂起，先写一条带时间戳的标记到 C:\。
REM 用于区分两种情况：
REM  A) C:\WinRE-PoC\winpeshl-boot.log 出现最新时间戳 -> winpeshl/cmd 正常进入，
REM     问题聚焦在 Recovery.exe 本身加载/挂起。
REM  B) 时间戳仍是旧的/缺失 -> 本次根本没进 winpeshl，Boot/WinRE 自动进入有问题。
echo WINPESHL-BOOT entered %date% %time% > C:\WinRE-PoC\winpeshl-boot.log
call "%~dp0RecoveryLauncher.cmd"
echo LAUNCHER-RETURNED %date% %time% >> C:\WinRE-PoC\winpeshl-boot.log