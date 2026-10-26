//! 失败回执的「错误码可见性」：让图形界面也能把 `BR-域-序号` 交到用户手里。
//!
//! ## 为什么需要这个模块
//!
//! 2.6.51 把 `error_code` 从 `online_operation` 深处带了出来（此前 `main` 用无码
//! `err()` 替换带码错误，导致 `--json` 的 `error_code` 恒 null）。但**图形界面侧
//! 一直没消费这个字段**：`WM_APP_ONLINE_DONE` 的弹窗只显示 `detail` 原文，
//! 状态栏更是一句固定的「在线任务失败，请检查日志」。
//!
//! 结果是一种很隐蔽的失效：`TaskError::Coded` 的 `Display` 自带 `[BR-…]` 前缀，
//! 所以**业务失败**看起来有码；而 `Io` / `Json` / `InvalidTransition` 这三类的
//! `Display` **故意不加前缀**（它们的文案是 OS 本地化串或 serde 原文，硬加反而
//! 更难读），它们的码只存在 `error_code` 字段里。于是「磁盘满、句柄占用」这类
//! 最常见、最需要查码的失败，用户在界面上一个码都看不到。
//!
//! ## 规则（三条，必须幂等）
//!
//! 1. 没有码 → 原样返回。panic 与「故意无码的用法错误」属于这一类，不能伪造码。
//! 2. 文字里**已经出现任何 `[BR-`** → 原样返回。真码已经在里面，再拼一个是
//!    两个码打架，比没码更糟（用户不知道该报哪个）。
//! 3. 其余 → `[码] ` 前置。
//!
//! ★ 幂等性不是洁癖：弹窗正文与日志行都要经过这里，若同一段文字被处理两次而
//! 结果不同，日志与界面就对不上了。

/// 码的合法前缀。本项目约定一律 `BR-<域>-<序号>`，其余字符串不当码用。
const CODE_PREFIX: &str = "BR-";

/// 把稳定错误码拼到失败回执前面（幂等；无合法码时原样返回）。
pub(crate) fn with_code(detail: &str, code: Option<&str>) -> String {
    let Some(code) = code else {
        return detail.to_string();
    };
    let code = code.trim();
    if code.is_empty() || !code.starts_with(CODE_PREFIX) {
        // 不是本项目的码：宁可原样，也不要往用户面前贴一个看起来像码的字符串。
        return detail.to_string();
    }
    if detail.contains("[BR-") {
        // 文字里已有码（`Coded` 变体的 Display、或上一轮已处理过）。
        return detail.to_string();
    }
    if detail.is_empty() {
        return format!("[{code}]");
    }
    format!("[{code}] {detail}")
}

#[cfg(test)]
mod tests {
    use super::with_code;

    #[test]
    fn no_code_keeps_text_untouched() {
        // panic 与用法错误本该无码，绝不能凭空造一个。
        assert_eq!(with_code("线程异常退出", None), "线程异常退出");
    }

    #[test]
    fn code_is_prepended_for_plain_text() {
        // Io 类失败的典型形状：详情里没有码，码只在字段里。
        assert_eq!(
            with_code("无法创建日志目录：磁盘已满", Some("BR-IO-001")),
            "[BR-IO-001] 无法创建日志目录：磁盘已满"
        );
    }

    #[test]
    fn existing_code_is_not_duplicated() {
        // ★ 最要紧的一条：`Coded` 的 Display 已带 `[BR-APL-001]`，
        // 再拼一次会变成两个码。判据：结果与原串逐字相同。
        let already = "[ERROR] 在线操作未完成：[BR-APL-001] DISM 超时";
        assert_eq!(with_code(already, Some("BR-APL-001")), already);
    }

    #[test]
    fn any_br_code_in_text_suppresses_prefixing() {
        // 文字里是**另一个**码也不加：说明真码已在内层错误里，
        // 外层字段只是兜底，硬加会让用户报错两个号。
        let inner = "失败：[BR-PE-039] 镜像不存在";
        assert_eq!(with_code(inner, Some("BR-IO-001")), inner);
    }

    #[test]
    fn idempotent_under_repeated_application() {
        let once = with_code("磁盘已满", Some("BR-IMG-008"));
        let twice = with_code(&once, Some("BR-IMG-008"));
        assert_eq!(once, twice);
    }

    #[test]
    fn empty_detail_yields_bare_code() {
        assert_eq!(with_code("", Some("BR-IO-001")), "[BR-IO-001]");
    }

    #[test]
    fn non_br_strings_are_not_treated_as_codes() {
        // 防御：拿到的不是本项目码时不得当前缀用。
        assert_eq!(with_code("失败", Some("2")), "失败");
        assert_eq!(with_code("失败", Some("")), "失败");
        assert_eq!(with_code("失败", Some("   ")), "失败");
    }
}
