# 重建 BackupRestore 测试盘（删除后可一键重建）

> 用途：磁盘 1（backuprestore-full-e2e，24GB）与磁盘 2（backuprestore-blank-compare，20GB）
> 是「备份还原 + PE 恢复」的测试沙盘（F: 备份源 / H: 镜像 / E: 测试 ESP / G:、P:、Q: 目标卷）。
> 删掉释放约 44GB 虚拟磁盘空间，需要测试时按本文件重建，全程约 5 分钟。

## 一、宿主机（macOS）：新建虚拟磁盘并附加到 VM

```bash
# 进 VM 包目录
cd "/Users/x/Parallels/Windows 11.pvm"

# 新建 24GB 测试盘（对应磁盘 1）
prlctl set "Windows 11" --device-add hdd --image "/Users/x/Parallels/Windows 11.pvm/backuprestore-full-e2e.hdd" --size 24576

# 新建 20GB 对比盘（对应磁盘 2）
prlctl set "Windows 11" --device-add hdd --image "/Users/x/Parallels/Windows 11.pvm/backuprestore-blank-compare.hdd" --size 20480
```

> 备选：Parallels 图形界面 → 虚拟机配置 → 硬件 → 添加硬盘（外部 .hdd，大小同上），
> 再启动 VM 即可。若 prlctl 的 `--device-add hdd` 参数与当前版本不一致，以 GUI 为准。

## 二、VM 内（Windows 11）：初始化分区

```cmd
copy /y \\Mac\backupRestore\tools\create-test-disks.ps1 C:\Users\Public\backupRestore-package\
powershell -ExecutionPolicy Bypass -File C:\Users\Public\backupRestore-package\create-test-disks.ps1
type C:\Users\Public\backupRestore-package\create-test-disks.log
```

脚本按磁盘型号自动匹配，重建结果：

| 磁盘 | 分区结构 |
|---|---|
| 磁盘 1（full-e2e，24GB） | MSR 16M + E:BREFI(FAT32,300M,EFI) + F:BRSource(NTFS,8G) + G:(NTFS,8G) + H:BRImages(NTFS,余量) |
| 磁盘 2（blank-compare，20GB） | MSR 16M + P:(NTFS,8G) + Q:(NTFS,8G) |

## 三、验证

```powershell
Get-Partition | Sort-Object DiskNumber,PartitionNumber | Format-Table DiskNumber,PartitionNumber,DriveLetter,Size,Type -AutoSize
Get-Volume | Where-Object DriveLetter | Format-Table DriveLetter,FileSystemLabel,FileSystem,Size -AutoSize
```

## 四、备注

- 测试 ESP（E:BREFI）为空盘，PE 恢复「硬盘启动」功能会在安装时自动写入 BCD 启动项；
  若需手动验证 bootmgr 引导链，可先把主系统盘 ESP（磁盘 0 分区 2）的
  `\EFI\Microsoft\Boot` 结构复制过来再测试。
- 清理历史目录（C:\Users\Public\backupRestore-src*、backupRestore-package-v1~v11）见
  docs/backuprestore-pe.md「v1.5.3 之后：清理开发残留」。
