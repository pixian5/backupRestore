//! 进程级 DPI 感知（System aware）+ 布局缩放系数（2.6.79 起）。
//!
//! ## 两个阶段，别混
//!
//! 1. **感知**（2.6.78）：`init()` 调 `SetProcessDPIAware()`，让窗口按物理像素绘制，
//!    不再被系统位图拉伸。这修好了「200% 下底部按钮落在窗口外、看不见」。
//!    代价是布局坐标（按 96dpi 写死）不再被放大 → **界面整体偏小**。
//! 2. **缩放**（2.6.79，本文件新增）：所有 96dpi 基准的布局值 × `scale()` 后再交给
//!    Win32。字体与坐标同步缩放、比例不变，所以既不会文字压住按钮、也不会小得看不清。
//!
//! ## 为什么是「在 96dpi 空间算完再统一乘」
//!
//! 布局有两套坐标来源：静态创建（字面量）和动态重排（`layout_operation` 里
//! 530 行，用「实际客户宽 − 常量」做右对齐）。后者若直接在物理量上乘系数会算错
//! （客户区已经是放大后的值，再整体放大就翻倍了）。
//! 所以约定：**所有布局计算都在 96dpi 基准空间里做**——
//! 动态重排先用 `unscale()` 把实际客户区换算回 96dpi，常量一概不动，
//! 最后在 `reposition` 里统一 `s()` 成物理像素。
//!
//! ★ 由此得到一条硬性质：**100% 缩放下 `scale() == 1.0`，`s(v) == v`、
//! `unscale(v) == v`，所有窗口尺寸与改动前逐像素等价**。
//! 这既是本改动的安全网，也是回归判据（100% 下几何判据必须仍全绿）。
//!
//! `init()` 必须在任何窗口创建之前调用（`main()` 第一件事）。

/// 布局缩放系数（1.0 = 96dpi / 100%，2.0 = 192dpi / 200%）。
///
/// 取自系统 DPI（声明感知后 `GetDeviceCaps` 返回的就是真实系统 DPI，不再是虚拟化值 96）。
/// 结果缓存：每次创建控件都取一次 DC 太浪费，且 DPI 在 System aware 下进程内不变。
/// 取不到或数值离谱（<48 或 >480）时退化为 1.0，即改动前的 96dpi 布局——宁可偏小，
/// 也不要因为读到一个荒谬的 DPI 而把界面画到屏幕外。
#[cfg(windows)]
pub(crate) fn scale() -> f32 {
    use std::sync::OnceLock;
    static SCALE: OnceLock<f32> = OnceLock::new();
    *SCALE.get_or_init(|| {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetDC(hwnd: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
            fn ReleaseDC(hwnd: *mut core::ffi::c_void, hdc: *mut core::ffi::c_void) -> i32;
        }
        #[link(name = "gdi32")]
        unsafe extern "system" {
            fn GetDeviceCaps(hdc: *mut core::ffi::c_void, index: i32) -> i32;
        }
        const LOGPIXELSY: i32 = 90;
        unsafe {
            let hdc = GetDC(core::ptr::null_mut());
            if hdc.is_null() {
                return 1.0;
            }
            let dpi = GetDeviceCaps(hdc, LOGPIXELSY);
            ReleaseDC(core::ptr::null_mut(), hdc);
            // 合理区间 48~480 dpi（50%~500%）；越界视为读数不可信，按 96dpi 处理。
            if !(48..=480).contains(&dpi) {
                return 1.0;
            }
            (dpi as f32 / 96.0).clamp(1.0, 5.0)
        }
    })
}

/// 96dpi 基准的布局值 → 实际物理像素。
#[cfg(windows)]
pub(crate) fn s(v: i32) -> i32 {
    (v as f32 * scale()).round() as i32
}

/// 实际物理像素 → 96dpi 基准值。
///
/// 只用于把 `GetClientRect` 读到的真实客户区换算回布局计算所用的基准空间，
/// 让「客户宽 − 常量」这类右对齐算式在两种缩放下都成立。
#[cfg(windows)]
pub(crate) fn unscale(v: i32) -> i32 {
    (v as f32 / scale()).round() as i32
}

// 非 Windows 平台：native_gui 本就编译不到，这里给出恒等实现以便其它代码引用。
#[cfg(not(windows))]
pub(crate) fn s(v: i32) -> i32 {
    v
}

#[cfg(not(windows))]
pub(crate) fn unscale(v: i32) -> i32 {
    v
}

#[cfg(windows)]
pub(crate) fn init() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetProcessDPIAware() -> i32;
    }
    // 失败不影响功能：保持 unaware（即 2.6.78 之前的现状，界面被系统拉伸）。
    unsafe {
        let _ = SetProcessDPIAware();
    }
}

// ===================== 界面字号（用户可在「程序设置」里自选） =====================
//
// 2.6.81 起字号不再写死：用户能在设置窗口里选 12 / 14 / 16 / 18，
// 选完立即生效（重设所有控件字体并重排），同时存进 `BackupRestore.settings.ini`。
//
// ★ 字号一变，"行距"和"按行数算出来的控件高度"必须跟着变，否则多行文本
//   的最后一行会被切成半截。所以这两个量**不许再写死**，一律走下面两个函数：
//   `line_height()` 与 `text_block_height()`。2026-10-26 踩过写死行距的坑，
//   2.6.80 又踩了一次（改字号没同步改高度），这是第三次被同一类问题咬到。

/// 设置文件里的键名（小写，与 `failure_threshold` 等保持一致）。
const FONT_SIZE_KEY: &str = "font_size";

/// 默认字号。14 = 2.6.80 定的基准（12 对中文偏小）。
pub(crate) const DEFAULT_FONT_SIZE: u32 = 14;

/// 可选档位（小 / 标准 / 大 / 特大）。设置窗口的下拉框按这个顺序画。
pub(crate) const FONT_SIZE_CHOICES: [u32; 4] = [12, 14, 16, 18];

/// 合法区间：太小看不清，太大会把布局撑出窗口。
const FONT_SIZE_MIN: u32 = 10;
const FONT_SIZE_MAX: u32 = 24;

static FONT_SIZE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(DEFAULT_FONT_SIZE);

/// 当前界面字号（像素，96dpi 基准）。
pub(crate) fn font_size() -> u32 {
    let v = FONT_SIZE.load(std::sync::atomic::Ordering::SeqCst);
    if (FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&v) {
        v
    } else {
        DEFAULT_FONT_SIZE
    }
}

/// 改用新字号（设置窗口保存后调用）。越界值被拒绝，返回是否真的改了。
pub(crate) fn set_font_size(v: u32) -> bool {
    if !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&v) {
        return false;
    }
    let old = FONT_SIZE.swap(v, std::sync::atomic::Ordering::SeqCst);
    old != v
}

/// 一行的高度：字号 + 外部行距。
///
/// 实测锚点：12 → 14（tmHeight 12 + tmExternalLeading 2）、14 → 17（14 + 3）。
/// 用 `(size + 5) / 6` 复现这两点，并在 16/18 上给出 19/21。
pub(crate) fn line_height() -> i32 {
    let s = font_size() as i32;
    s + (s + 5) / 6
}

/// 多行文本控件需要的高度 = 行数 × 行距 + 附加量（边框差 + 余量）。
///
/// 2.6.80 的两个锚点：详情框 10 行 → `10×17 + 8 = 178`；
/// 状态框 9 行 → `9×17 + 25 = 178`（该控件的文本区比控件高少 14px，另留 11px 余量）。
pub(crate) fn text_block_height(rows: i32, extra: i32) -> i32 {
    rows * line_height() + extra
}

/// 启动时从设置文件读字号。文件不存在 / 值非法 → 默认值（界面必须能起来）。
pub(crate) fn load_font_size(executable_dir: &std::path::Path) -> u32 {
    use std::fs;
    let path = executable_dir.join(super::settings_file::SETTINGS_FILE_NAME);
    let Ok(text) = fs::read_to_string(path) else {
        return DEFAULT_FONT_SIZE;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == FONT_SIZE_KEY {
            if let Ok(v) = value.trim().parse::<u32>() {
                if (FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&v) {
                    return v;
                }
            }
        }
    }
    DEFAULT_FONT_SIZE
}

/// 把字号写进设置文件（**合并**，只替换 `font_size` 一个键）。
///
/// 为什么必须合并：`BackupRestore.settings.ini` 是共享文件，`failure_policy`
/// 与 `dism_timeout` 也往里写。整文件重写会把它们的键**静默**冲掉，
/// 用户改一次字号就丢失失败策略——这种丢失事后极难归因（`failure_policy`
/// 的注释里记过同样的教训）。
pub(crate) fn save_font_size(
    executable_dir: &std::path::Path,
    size: u32,
) -> std::io::Result<()> {
    use std::fs;
    use std::io::Write;
    if !(FONT_SIZE_MIN..=FONT_SIZE_MAX).contains(&size) {
        return Ok(());
    }
    fs::create_dir_all(executable_dir)?;
    let path = executable_dir.join(super::settings_file::SETTINGS_FILE_NAME);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut replaced = false;
    for line in existing.lines() {
        let trimmed = line.trim();
        let is_font_key = !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with(';')
            && trimmed.split_once('=').map(|(k, _)| k.trim() == FONT_SIZE_KEY)
                == Some(true);
        if is_font_key {
            if !replaced {
                out.push_str(&format!("{FONT_SIZE_KEY} = {size}\n"));
                replaced = true;
            }
            // 重复的旧行直接丢弃，避免留下两行 font_size。
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("{FONT_SIZE_KEY} = {size}\n"));
    }
    let mut file = fs::File::create(path)?;
    file.write_all(out.as_bytes())
}
