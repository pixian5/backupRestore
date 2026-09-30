# wim-info 优化：sidecar 优先读取 与 --skip-hash 开关（v1.9.5）

**时间**：2026-09-30 18:30  
**模型**：Gemini 2.5 Pro  
**版本**：1.9.4 → 1.9.5  

---

## 背景与问题

原 `wim-info` 命令每次调用都：
1. 启动 DISM `/Get-WimInfo`（需等待进程返回）
2. 对整个 WIM 文件流式计算 SHA-256（62GB WIM 约需数分钟）

对于 GUI 的「镜像信息快速预览」场景，这个耗时是不可接受的。

## 解决方案

### 1. Sidecar 优先读取（毫秒级）

`wim-info` 现在优先扫描同目录的 `*.index-N.metadata.json` sidecar 文件：

- 命名规则：`<wim文件名>.index-<N>.metadata.json`（例如 `6.wim.index-1.metadata.json`）
- 若存在，直接反序列化输出，**完全跳过 DISM 和哈希计算**
- 输出 JSON 中增加 `"source": "sidecar"` 标记
- 多索引时按索引号升序排列，一次输出全部

### 2. --skip-hash 开关

当无 sidecar 时回退 DISM 模式，此时支持 `--skip-hash`：
- 有此标志：跳过 SHA-256 流式计算，`sha256` 字段返回 `"skipped"`
- 无此标志：保持原行为，完整计算哈希（适合严格校验/还原前确认）

### 命令格式

```
BackupRestore.exe wim-info <absolute-wim> [--skip-hash]
```

## 实测验证结果

```
F:\6.wim（58GB）+ F:\6.wim.index-1.metadata.json 均存在

wim-info F:\6.wim --skip-hash  → source:sidecar, 耗时<100ms（Start-Process 约1s）
wim-info F:\6.wim              → source:sidecar, 耗时<100ms（同上）
```

输出示例（sidecar 路径）：
```json
{
  "image": "F:\\6.wim",
  "sha256": "c0509d19b8539ee8d5d4091514b1b1e75c8f4a44bdb24fb2e778ff66a47bc979",
  "source": "sidecar",
  "images": [{
    "index": 1,
    "name": "Windows Backup (index 1)",
    "imageSize": 62327010250,
    "sha256": "c0509d19...",
    "createdAt": "2026-09-30T08:27:43Z",
    "computer": "P8B6",
    "capturedUsedBytes": 81900687360,
    "minimumTargetSize": 267861884928,
    "programVersion": "1.9.1",
    "source": "sidecar"
  }]
}
```

## 代码修改

| 文件 | 变更 |
|------|------|
| `crates/backuprestore-cli/src/windows_prepare.rs` | `wim_info()` 增加 `skip_hash: bool` 参数，添加 sidecar 扫描逻辑 |
| `crates/backuprestore-cli/src/main.rs` | `wim-info` 分支解析 `--skip-hash` 标志，更新 help 字符串 |

## 测试结果

- `cargo test --workspace`：全部通过
- Windows ARM64 编译：1,865,216 字节，部署哈希 `193ddd34...`
- 实机验证：sidecar 优先读取正确，JSON 字段完整
