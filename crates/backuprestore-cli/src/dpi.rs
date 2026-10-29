//! 进程级 DPI 感知（System aware）。
//!
//! 只做「让窗口按物理像素绘制」，**不做布局缩放**——布局缩放是下一步。
//! 当前作用：200% 缩放下 DPI-unaware 的窗口被系统位图放大，客户区坐标被
//! 虚拟化，底部控件落在窗口外（2026-10-30 实测：按钮 y=595 在报告客户区
//! 898 之内，但截图根本看不到；「保存位置」对话框的按钮同样被裁）。
//! 100% 缩放下本改动不改变任何窗口尺寸。
//!
//! 必须在任何窗口创建之前调用（`main()` 第一件事）。

#[cfg(windows)]
pub(crate) fn init() {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetProcessDPIAware() -> i32;
    }
    // 失败不影响功能：保持 unaware（即改动前的现状）。
    unsafe {
        let _ = SetProcessDPIAware();
    }
}
