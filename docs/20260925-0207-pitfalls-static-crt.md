# 踩坑 25：动态 CRT 让程序在 WinRE / 精简 Windows 上直接无法加载

- 日期：2026-09-25
- 版本：1.6.7
- 症状：把 `BackupRestore.exe` 复制到干净的 tiny11 (ARM64) 后，所有命令都**零输出、
  零副作用**；`Start-Process -Wait` 拿到的退出码是 `4294967418`
  （= `0xC0000135` = `STATUS_DLL_NOT_FOUND`）。连 GUI 都起不来。

## 为什么之前没发现

在完整版 Windows 11 虚拟机里 `-C target-feature` 保持默认，产物动态链接 MSVC 运行库，
系统自带 `VCRUNTIME140.dll` 与 UCRT，一切正常。**WinRE / WinPE / 精简版 Windows 没有这些
运行库**，程序在进程创建阶段就被加载器终止，连 `main()` 都进不去，所以没有任何日志、
没有 `logs/launcher-errors.log`、也没有错误弹窗。

排查时最容易走的弯路：**怀疑是环境缺一堆系统 DLL**。实测 tiny11 的
`C:\Windows\System32` 里 `api-ms-win-*.dll` 一个都没有
（`dir /b %windir%\System32\api-ms-win-*.dll | find /c /v ""` = 0），看起来像主因。
但这条路是错的 —— `api-ms-win-*.dll` 属于 **API Set**，由内核的 ApiSetSchema 解析，
磁盘上是否有同名文件不影响加载。真正的加载失败项只有 CRT：

```
$ python3 -c "解析 PE 导入表"
IMPORT DLLS: USER32, GDI32, COMDLG32, KERNEL32, ADVAPI32, SHELL32, ole32,
             COMCTL32, api-ms-win-core-synch-l1-2-0, ntdll, bcryptprimitives,
             VCRUNTIME140, api-ms-win-crt-runtime-l1-1-0, api-ms-win-crt-stdio-l1-1-0,
             api-ms-win-crt-math-l1-1-0, api-ms-win-crt-locale-l1-1-0,
             api-ms-win-crt-heap-l1-1-0
```

排查顺序应该是：**先看退出码 / 再看 PE 导入表 / 最后才怀疑运行环境**。退出码
`0xC0000135` 已经把结论摆在脸上了。

## 修复

`build-win.sh` 的 `RUSTFLAGS` 加 `-C target-feature=+crt-static`：

```bash
RUSTFLAGS="-C target-feature=+crt-static -C link-arg=/LIBPATH:... " \
cargo build --release --target aarch64-pc-windows-msvc -p backuprestore-cli
```

修复后导入表只剩 11 项系统 DLL，`VCRUNTIME140.dll` 与全部 `api-ms-win-crt-*` 消失：

```
IMPORT DLLS: USER32.dll, GDI32.dll, COMDLG32.dll, KERNEL32.dll, ADVAPI32.dll,
             SHELL32.dll, ole32.dll, COMCTL32.dll, api-ms-win-core-synch-l1-2-0.dll,
             ntdll.dll, bcryptprimitives.dll
RESIDUAL_CRT_DEPS: NONE
```

产物体积：1,531,904 → 1,628,672 字节（静态库进来的那点体积，换零外部依赖，值）。

链接时会出现 LNK4099 警告（`libcmt.lib` / `libvcruntime.lib` 找不到配对的 PDB），
这是 Windows SDK 静态库自带的正常噪声，不影响产物。

## 连带收益：WinRE payload 不再需要搬运运行库

`crates/backuprestore-cli/src/winre_payload.rs` 里的 `OPTIONAL_RUNTIME_FILES`
原本会把 `VCRUNTIME140.dll` / `VCRUNTIME140_1.dll` 一起拷进 WIM。静态链接后这两个文件
通常不在程序目录里，`copy_required` 的 `if source.is_file()` 判断会自然跳过，
注入的 `Recovery.exe` 本身就已经自包含。**这段"可选运行库"逻辑保留即可，但已经不再是
WinRE 能跑起来的前提条件。**

## 验证记录（tiny11 / ARM64 / 10.0.26200.9457）

```
> BackupRestore.exe list-volumes
[{"letter":"C","filesystem":"NTFS","diskNumber":0,"partitionNumber":3,
  "hasWindowsInstallation":true, ...}]

> BackupRestore.exe inspect-environment
{"architecture":"arm64","winreAvailable":true,"windows":"Windows 10.0.26200.9457", ...}
```

修复前同样的命令是 `exit=-1073741515` + 完全没有输出。

## 复用到别的场景

判断任意 Rust/MSVC 产物是不是被这个坑卡住，三步：

1. `(& .\X.exe arg).ExitCode` 或 `Start-Process -Wait -PassThru`，看是不是 `-1073741515`；
2. 解析 PE 导入表，看有没有 `VCRUNTIME*.dll` / `api-ms-win-crt-*`；
3. 有就加 `-C target-feature=+crt-static`，重新构建，再比对导入表。
