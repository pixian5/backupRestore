# 构建辅助工具

本目录仅保留 Windows ADK/WinPE 构建辅助工具：

- `build-backuprestore-pe.ps1`：把 Rust `Recovery.exe` 和独立 PE 模板写入 ADK
  ARM64 WinPE WIM，并重新挂载核验。
- `elevate-build.rs`：仅为上一个构建脚本请求 UAC 的开发辅助程序。

产品运行时不从这里加载任何文件。旧的 `startnet.cmd`、`RecoveryPoC.cmd`、共享
`winpeshl.ini`、自动注入和手工恢复脚本已删除，避免它们被误当作当前 WinRE 入口。
