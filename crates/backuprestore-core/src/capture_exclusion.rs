//! WIM 备份捕获排除列表（Capture Exclusion List）生成与配置管理。
//!
//! 在通过 DISM 备份捕获时，排除页面文件、系统还原点、临时目录、更新缓存、
//! 浏览器媒体缓存、Parallels 独占占位文件及程序自身部署件等可再生或破坏性文件。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::TaskError;

/// 程序自身安装目录（exe 所在目录）里可再生的部署件。
///
/// 只排这几个**文件**，不排整个目录：同目录下的 `tasks\`（任务证据）与
/// `logs\`（日志）必须留在镜像里——2026-10-24 用户拍板「证据全留」，
/// 而二进制是可再生过程产物，且会被 A 入口还原系统盘时随镜像回滚成旧版
/// （实测过：镜像元数据 programVersion 偏低、自启 GUI 标题 v2.6.31）。
pub const SELF_DEPLOY_FILES: &[&str] = &[
    "BackupRestore.exe",
    "Recovery.exe",
    "wimlib-imagex.exe",
    "libwim-15.dll",
    "winpe-winpeshl.ini",
];

/// 程序的历史分发目录（2026-10-24 起已不再部署，deploy 会 rmdir 清残留）。
/// 固定排除：旧镜像还原后会把它带回来，排除可让"还原后不复发"，
/// 与 deploy 的 rmdir 形成双保险。
pub const LEGACY_PACKAGE_DIR: &str = "\\Users\\Public\\backupRestore-package";

/// 取路径的卷键（盘符，大写，形如 `H:`）；非 `X:\...` 形式（UNC、Unix 路径等）
/// 返回 None。
///
/// 手写前两字符解析而不用 `Component::Prefix`：后者只在 Windows target 上
/// 产生，而单元测试常在 macOS 宿主上跑（本项目交叉编译），必须行为一致。
pub fn volume_key(p: &Path) -> Option<String> {
    let s = p.as_os_str().to_string_lossy();
    let mut chars = s.chars();
    let first = chars.next()?;
    let second = chars.next()?;
    if first.is_ascii_alphabetic() && second == ':' {
        Some(s[..2].to_ascii_uppercase())
    } else {
        None
    }
}

/// 把绝对路径转成「卷内相对路径」（反斜杠连接）；exe 就在卷根、或路径含
/// 非规范段（空段/`.`/`..`）时返回 None。
pub fn relative_to_volume_root(p: &Path) -> Option<String> {
    let s = p.as_os_str().to_string_lossy().replace('/', "\\");
    let (drive, rest) = (s.get(..2)?, s.get(2..)?);
    if !drive.chars().next()?.is_ascii_alphabetic() || !drive.ends_with(':') {
        return None;
    }
    let rest = rest.strip_prefix('\\')?;
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split('\\') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("\\"))
}

/// 该路径是不是**重解析点**（符号链接 / 目录联接点），且不跟随它。
///
/// Windows 侧必须看 `FILE_ATTRIBUTE_REPARSE_POINT`：Rust 的
/// `FileType::is_symlink()` 对目录联接点（IO_REPARSE_TAG_MOUNT_POINT）返回 false，
/// 而 Windows 卷根上的 `Documents and Settings` 正是联接点，用 `is_symlink()`
/// 会漏判。非 Windows 侧没有联接点概念，用符号链接近似即可（只为单测可跑）。
pub fn is_reparse_point(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        fs::symlink_metadata(path)
            .map(|m| m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        fs::symlink_metadata(path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
    }
}

/// 任务目录里的 `status.json` 是否已经处于**终态**（`success` / `failed`）。
///
/// 读不到、解析失败、字段缺失、目录里根本没有 `status.json` 一律按**非终态**处理
/// —— 宁可多排掉一个目录（少备份一条"没跑完"的记录），也不要让半截任务进镜像：
/// 它会带 1.4GB 的恢复环境镜像，并让还原后的新系统弹一个续不了的续跑框。
fn task_stage_is_terminal(task_dir: &Path) -> bool {
    #[derive(serde::Deserialize)]
    struct Probe {
        stage: String,
    }
    let Ok(text) = fs::read_to_string(task_dir.join("status.json")) else {
        return false;
    };
    let Ok(probe) = serde_json::from_str::<Probe>(&text) else {
        return false;
    };
    matches!(probe.stage.as_str(), "success" | "failed")
}

/// 列出 `tasks` 目录里**还没跑完**的任务名（非终态），供捕获排除表使用。
///
/// 单独抽出来是为了能在 macOS 上单测：`build_capture_exclusions` 里那段被
/// 「源卷与程序卷是否同一个」的判定把关，而那个判定在非 Windows 上恒为假
/// —— 直接测整函数会得到空列表，把「逻辑错」误判成「平台不同」。
pub fn pending_task_names(tasks_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(tasks_dir) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if task_stage_is_terminal(&entry.path()) {
            continue;
        }
        names.push(name);
    }
    names.sort();
    names
}

/// 生成 DISM `/ConfigFile` 配置文件内容（WimScript.ini 语法）。
///
/// 用途：备份捕获时可排除 \$Recycle.Bin、临时目录、更新缓存、浏览器缓存等
/// 可再生内容，缩小 WIM 镜像体积。
///
/// DISM 配置文件的排除表是"根路径锚定"的（带前导 `\` 只匹配根下精确路径），
/// 且通配符只能出现在"不以反斜杠开头的路径的最后一段"。因此 `\Users\*\
/// AppData\...` 这类中间通配符不在规范内，浏览器缓存目录必须在捕获时
/// 枚举真实的 `Users\<用户>\AppData\Local\...` 字面路径生成。
///
/// `source_root` 是待捕获卷的根（如 `X:\`，要求是物理卷根，枚举时不要求
/// 该卷可写）。返回的文本写入后传给：
/// `dism /Capture-Image ... /ConfigFile:<path>` 或
/// `dism /Append-Image ... /ConfigFile:<path>`。
///
/// `self_exe_dir` 是程序自身 exe 所在目录（调用方传
/// `current_exe().parent()`）：与 `source_root` 同卷时，把该目录下的
/// 部署件（[`SELF_DEPLOY_FILES`]）逐个排除。跨卷时无对应路径，不加项。
pub fn build_capture_exclusions(source_root: &Path, self_exe_dir: Option<&Path>) -> Result<String, TaskError> {
    // 固定的根级可排除项（全部为 DISM 规范内的根路径锚定写法，无通配符）。
    // 注意：hiberfil.sys/pagefile.sys/swapfile.sys/\System Volume Information
    // 由 DISM 默认排除，无需在此重复。
    let mut lines = vec![
        // Parallels 在每个卷根创建同名占位文件并**始终独占持有**它。它不属于
        // 用户数据，却会让 `dism /Apply-Image` 在重建该文件时以
        // ERROR_SHARING_VIOLATION(0x80070020) 失败（实测 v1.7.4 在 T: 还原
        // 72% 处中断）。DISM 在打开文件之前就按排除表跳过，因此排掉它即可
        // 免除"还原前必须停掉 Parallels Tools Service"这一前置条件。
        // 实测排除匹配不区分大小写（VERDICT=CASE_INSENSITIVE）。
        // 非 Parallels 环境不存在该条目，排除它无副作用。
        "\\Mac disk".to_string(),
        "\\$Recycle.Bin".to_string(),
        "\\$WINDOWS.~BT".to_string(),
        "\\$WINDOWS.~WS".to_string(),
        "\\Windows.old".to_string(),
        "\\Temp".to_string(),
        "\\Windows\\Temp".to_string(),
        "\\Windows\\SoftwareDistribution\\Download".to_string(),
        "\\Windows\\Prefetch".to_string(),
        "\\Windows\\Logs".to_string(),
        "\\Windows\\Panther".to_string(),
        "\\ProgramData\\Microsoft\\Windows\\WER".to_string(),
        // 程序自身的历史分发目录（2026-10-24 起不再部署；旧镜像还原会带回）
        LEGACY_PACKAGE_DIR.to_string(),
    ];

    // 根级自引用联接点（2026-10-26）。
    //
    // Windows 卷根上的 `Documents and Settings` 是 XP 兼容名，实际是一个指向
    // **本卷 `Users` 目录**的联接点。不排除它，捕获就会把它当作一棵真实子树再遍历
    // 一遍 `Users`——文件数凭空翻倍，预扫时间同比例拉长（而 DISM 的预扫发生在写第一
    // 个字节之前，用户看到的就是「一直没动静」）。
    //
    // ★ 只在**确认它是重解析点**时才排除。数据卷上万一真有一个叫这个名字的普通
    //   目录，无脑排除会静默丢掉用户数据——排除表的错误是"少备份"，不报错。
    const ROOT_JUNCTIONS: &[&str] = &["Documents and Settings"];
    for junction in ROOT_JUNCTIONS {
        if is_reparse_point(&source_root.join(junction)) {
            lines.push(format!("\\{junction}"));
        }
    }

    // 枚举真实用户配置文件，补上按用户字面路径的排除项。
    let users_root = source_root.join("Users");
    if let Ok(profiles) = fs::read_dir(&users_root) {
        let mut user_entries: Vec<String> = Vec::new();
        for profile in profiles.flatten() {
            // ★ 必须是目录：`C:\Users` 下还有一个 `desktop.ini` 文件，早期版本把它
            // 当成了用户名，于是排除表里出现了 `\Users\desktop.ini\AppData\Local\Temp`
            // 这种畸形条目（2026-10-26 在实机排除表里抓到）。
            if !profile.path().is_dir() {
                continue;
            }
            let Some(name) = profile.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let local = users_root.join(&name).join("AppData").join("Local");
            // 每个用户自己的临时目录
            user_entries.push(format!("\\Users\\{name}\\AppData\\Local\\Temp"));
            // Chromium 系浏览器的媒体缓存目录（User Data 下每个配置子目录）
            for child in [
                "Google\\Chrome",
                "Microsoft\\Edge",
                "BraveSoftware\\Brave-Browser",
                "Vivaldi",
            ] {
                let mut base = local.clone();
                for part in child.split('\\') {
                    base = base.join(part);
                }
                let user_data = base.join("User Data");
                for profile_dir in read_subdirs(&user_data) {
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Cache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Code Cache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\GPUCache"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Service Worker\\CacheStorage"
                    ));
                    user_entries.push(format!(
                        "\\Users\\{name}\\AppData\\Local\\{child}\\User Data\\{profile_dir}\\Service Worker\\ScriptCache"
                    ));
                }
            }
            // Opera：稳定配置文件直接位于 Opera Stable 下
            let opera = local.join("Opera Software").join("Opera Stable");
            if opera.is_dir() {
                user_entries.extend([
                    format!("\\Users\\{name}\\AppData\\Local\\Opera Software\\Opera Stable\\Cache"),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Opera Software\\Opera Stable\\GPUCache"
                    ),
                ]);
            }
            // Firefox：Profiles 下每个配置的缓存目录
            let firefox = local.join("Mozilla").join("Firefox").join("Profiles");
            for profile_dir in read_subdirs(&firefox) {
                user_entries.extend([
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\cache2"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\cache2\\entries"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\OfflineCache"
                    ),
                    format!(
                        "\\Users\\{name}\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\{profile_dir}\\startupCache"
                    ),
                ]);
            }
            // 旧版 IE / 系统组件缓存
            let inet_cache = local.join("Microsoft").join("Windows").join("INetCache");
            if inet_cache.is_dir() {
                user_entries.push(format!(
                    "\\Users\\{name}\\AppData\\Local\\Microsoft\\Windows\\INetCache"
                ));
            }
        }
        user_entries.sort();
        user_entries.dedup();
        lines.extend(user_entries);
    }

    // 程序自身部署件（exe 同目录的五个可再生文件）。只排文件、不排目录：
    // 同目录的 tasks\（任务证据）与 logs\ 必须留在镜像里（证据全留，2026-10-24 拍板）。
    if let Some(exe_dir) = self_exe_dir
        && let (Some(src_vol), Some(exe_vol)) = (volume_key(source_root), volume_key(exe_dir))
        && src_vol == exe_vol
        && let Some(rel) = relative_to_volume_root(exe_dir)
    {
        for file in SELF_DEPLOY_FILES {
            lines.push(format!("\\{rel}\\{file}"));
        }
        // ★ 2.7.2：**未完成任务的工作树不进镜像**。
        for name in pending_task_names(&exe_dir.join("tasks")) {
            lines.push(format!("\\{rel}\\tasks\\{name}"));
        }
    }

    let mut text = String::from("[ExclusionList]\r\n");
    for line in lines {
        text.push_str(&line);
        text.push_str("\r\n");
    }
    Ok(text)
}

/// 生成 DISM 排除配置并落盘，返回可传给 `/ConfigFile:` 的路径。
///
/// **三个捕获入口必须共用本函数**：CLI 备份、GUI 在线备份、PE 备份。
pub fn write_capture_exclusion_config(source_root: &Path, self_exe_dir: Option<&Path>) -> Result<PathBuf, TaskError> {
    let text = build_capture_exclusions(source_root, self_exe_dir)?;
    let path = env::temp_dir().join("BackupRestore-exclusions.ini");
    fs::write(&path, text)?;
    Ok(path)
}

/// 列出 `root` 下的一级子目录名（失败时返回空），供枚举浏览器配置目录使用。
fn read_subdirs(root: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.push(name.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn capture_exclusions_include_fixed_and_per_user_browser_entries() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl-{suffix}"));
        let users = root.join("Users");
        let alice = users.join("alice").join("AppData").join("Local");
        fs::create_dir_all(
            alice
                .join("Google")
                .join("Chrome")
                .join("User Data")
                .join("Default")
                .join("Cache"),
        )
        .unwrap();
        fs::create_dir_all(
            alice
                .join("Mozilla")
                .join("Firefox")
                .join("Profiles")
                .join("abc.default")
                .join("cache2"),
        )
        .unwrap();

        let ini = build_capture_exclusions(&root, None).unwrap();
        let lines: Vec<&str> = ini.lines().collect();
        let list = lines
            .iter()
            .position(|l| *l == "[ExclusionList]")
            .expect("配置应包含 [ExclusionList] 节");
        let all: Vec<&str> = lines[list + 1..]
            .iter()
            .filter(|l| !l.is_empty())
            .copied()
            .collect();

        // 固定根级排除项
        for fixed in [
            "\\Mac disk",
            "\\$Recycle.Bin",
            "\\$WINDOWS.~BT",
            "\\Windows.old",
            "\\Temp",
            "\\Windows\\Temp",
            "\\Windows\\SoftwareDistribution\\Download",
            "\\Users\\Public\\backupRestore-package",
        ] {
            assert!(all.contains(&fixed), "缺少固定排除项 {fixed}: {ini}");
        }
        // 每个用户字面路径
        assert!(all.contains(&"\\Users\\alice\\AppData\\Local\\Temp"));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Cache"
        ));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Code Cache"
        ));
        assert!(all.contains(
            &"\\Users\\alice\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\abc.default\\cache2"
        ));
        assert!(all.contains(&"\\Users\\alice\\AppData\\Local\\Mozilla\\Firefox\\Profiles\\abc.default\\cache2\\entries"));
        // 规范内不得出现中间通配符（DISM 只允许最后一段通配）
        for entry in &all {
            let normalized = entry.replace('/', "\\");
            let parts: Vec<&str> = normalized.split('\\').collect();
            let (first, _) = parts.split_first().unwrap();
            if first.is_empty() {
                // 根路径锚定写法（\x\y）：除最后一段外其余段不得含 *
                if let Some((head, tail)) = parts.split_first() {
                    assert!(!head.contains('*'), "根锚定路径的首段不应含通配符: {entry}");
                    for part in tail.split_last().unwrap().1 {
                        assert!(
                            !part.contains('*'),
                            "根锚定路径的中间段不应含通配符: {entry}"
                        );
                    }
                }
            }
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_exclusions_cover_root_junction_and_skip_non_directory_profiles() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl-junction-{suffix}"));
        let users = root.join("Users");
        fs::create_dir_all(users.join("alice").join("AppData").join("Local")).unwrap();
        // 非目录条目：不能被当成用户名。
        fs::write(users.join("desktop.ini"), "[.ShellClassInfo]").unwrap();

        // 没有联接点时，绝不能排除同名真实目录——排除表的错误是「少备份」，不报错。
        fs::create_dir_all(root.join("Documents and Settings")).unwrap();
        let ini = build_capture_exclusions(&root, None).unwrap();
        assert!(
            !ini.contains("\\Documents and Settings"),
            "普通同名目录不能被排除，否则静默丢用户数据: {ini}"
        );
        assert!(
            !ini.contains("\\Users\\desktop.ini\\"),
            "desktop.ini 是文件、不是用户: {ini}"
        );
        assert!(ini.contains("\\Users\\alice\\AppData\\Local\\Temp"));

        // 换成真正指向 Users 的联接点后，必须被排除。
        fs::remove_dir_all(root.join("Documents and Settings")).unwrap();
        let linked = link_dir(&users, &root.join("Documents and Settings"));
        if linked {
            let ini = build_capture_exclusions(&root, None).unwrap();
            assert!(
                ini.contains("\\Documents and Settings"),
                "自引用联接点必须排除: {ini}"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    /// 建目录链接（Windows 上退化为目录联接点）。不支持时返回 false，测试跳过。
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
    }

    #[test]
    fn capture_exclusions_without_users_keeps_fixed_list() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("backuprestore-excl2-{suffix}"));
        let ini = build_capture_exclusions(&root, None).unwrap();
        assert!(ini.starts_with("[ExclusionList]\r\n"));
        assert!(ini.contains("\r\n\\Mac disk\r\n"));
        assert!(ini.contains("\r\n\\$Recycle.Bin\r\n"));
        assert!(!ini.contains("AppData\\"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_capture_exclusion_config_lands_file_with_parallels_placeholder() {
        let root = std::env::temp_dir().join(format!(
            "backuprestore-excl3-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = write_capture_exclusion_config(&root, None).unwrap();
        assert_eq!(path.file_name().unwrap(), "BackupRestore-exclusions.ini");
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\r\n\\Mac disk\r\n"),
            "配置应排除卷根占位符: {text}"
        );
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn capture_exclusions_skip_self_deploy_files_same_volume() {
        let root = Path::new("H:\\");
        let exe_dir = Path::new("H:\\brwork");

        let ini = build_capture_exclusions(root, Some(exe_dir)).unwrap();
        for file in [
            "BackupRestore.exe",
            "Recovery.exe",
            "wimlib-imagex.exe",
            "libwim-15.dll",
            "winpe-winpeshl.ini",
        ] {
            assert!(
                ini.contains(&format!("\\brwork\\{file}\r\n")),
                "应排除同卷自身部署件 {file}: {ini}"
            );
        }
        assert!(
            !ini.contains("\r\n\\brwork\r\n"),
            "不得整目录排除自身安装目录（tasks/logs 证据要留）: {ini}"
        );

        let ini2 = build_capture_exclusions(root, Some(Path::new("C:\\brwork"))).unwrap();
        assert!(
            !ini2.contains("\\brwork\\BackupRestore.exe"),
            "跨卷不应生成自身部署件排除项: {ini2}"
        );

        let ini3 = build_capture_exclusions(root, Some(Path::new("H:\\"))).unwrap();
        assert!(
            !ini3.contains("\r\n\\BackupRestore.exe\r\n"),
            "exe 在卷根时不应生成排除项: {ini3}"
        );
    }

    #[test]
    fn pending_tasks_excluded_finished_kept() {
        let tasks = std::env::temp_dir().join(format!(
            "br-excl-tasks-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        for (id, stage) in [
            ("done-1", Some(r#"{"stage":"success"}"#)),
            ("fail-1", Some(r#"{"stage":"failed"}"#)),
            ("busy-1", Some(r#"{"stage":"capturing"}"#)),
            ("broken-1", None),
        ] {
            let dir = tasks.join(id);
            fs::create_dir_all(&dir).unwrap();
            if let Some(text) = stage {
                fs::write(dir.join("status.json"), text).unwrap();
            }
        }

        let pending = pending_task_names(&tasks);
        assert_eq!(
            pending,
            vec!["broken-1".to_string(), "busy-1".to_string()],
            "只应排除非终态：终态 success/failed 保留，读不到的按非终态排除"
        );

        let _ = fs::remove_dir_all(&tasks);
    }
}
