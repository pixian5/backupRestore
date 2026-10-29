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
