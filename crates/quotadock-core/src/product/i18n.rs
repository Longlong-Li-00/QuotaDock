//! Text produced by Rust (tray tooltip/menu, Windows notifications) in English and Chinese.
//! The webview has its own typed dictionaries (ui/src/i18n); wording is kept identical.

use super::settings::Lang;

pub struct Strings {
    pub app: &'static str,
    pub paused_title: &'static str,
    pub five_hour: &'static str,
    pub weekly: &'static str,
    pub codex: &'static str,
    pub menu_open: &'static str,
    pub menu_refresh: &'static str,
    pub menu_pause: &'static str,
    pub menu_resume: &'static str,
    pub menu_quit: &'static str,
    pub availability_unknown: &'static str,
    pub availability_unknown_persistent: &'static str,
    pub limit_reached: &'static str,
    pub not_signed_in: &'static str,
    /// `not_signed_in` where the full words do not fit: the widget's compact name slot.
    pub not_signed_in_short: &'static str,
    pub codex_not_found: &'static str,
    pub provider_unavailable: &'static str,
    /// `provider_unavailable` where the full words do not fit: the widget's compact name slot.
    pub provider_unavailable_short: &'static str,
    /// Codex found, but only a version older than the verified one (the Overview's words).
    pub codex_unverified: &'static str,
    pub codex_unverified_short: &'static str,
    pub no_data_yet: &'static str,
    pub close_to_tray_title: &'static str,
    pub close_to_tray_body: &'static str,
    // notifications
    pub n_available_title: &'static str,
    pub n_available_body: &'static str,
    pub n_credit_title: &'static str,
    pub n_credit_body: &'static str,
    pub n_refresh_title: &'static str,
    pub n_allowance_title: &'static str,
    pub n_allowance_body: &'static str,
    pub n_low_combined_title: &'static str,
    pub n_forecast_title: &'static str,
    pub n_forecast_body: &'static str,
    pub n_provider_error_title: &'static str,
    pub n_provider_error_body: &'static str,
    pub n_also: &'static str,
    pub n_test_title: &'static str,
    pub n_test_body: &'static str,
}

pub const EN: Strings = Strings {
    app: "QuotaDock",
    paused_title: "QuotaDock — Paused",
    five_hour: "5h",
    weekly: "Weekly",
    codex: "Codex",
    menu_open: "Open QuotaDock",
    menu_refresh: "Refresh now",
    menu_pause: "Pause monitoring",
    menu_resume: "Resume monitoring",
    menu_quit: "Quit",
    availability_unknown: "Availability unknown",
    availability_unknown_persistent: "Availability has remained unknown",
    limit_reached: "Limit reached",
    not_signed_in: "Sign in to Codex first",
    not_signed_in_short: "Sign in first",
    codex_not_found: "Codex installation not found",
    provider_unavailable: "Cannot refresh Codex usage",
    provider_unavailable_short: "Can't refresh",
    codex_unverified: "Codex version not yet verified",
    codex_unverified_short: "Unverified",
    no_data_yet: "Connecting to Codex…",
    close_to_tray_title: "QuotaDock is still running",
    close_to_tray_body: "QuotaDock will continue running in the system tray.",
    n_available_title: "Codex is available again",
    n_available_body: "Codex reports that ordinary usage is allowed again.",
    n_credit_title: "Codex reset credit used",
    n_credit_body: "Your Codex quota changed.\n1 reset credit was used.",
    n_refresh_title: "Codex quota reset early",
    n_allowance_title: "Codex quota reset",
    n_allowance_body: "The previous quota period ended.",
    n_low_combined_title: "Codex quota is running low",
    n_forecast_title: "Codex quota may run out early",
    n_forecast_body: "QuotaDock estimates the current pace may use up the quota before the next confirmed reset.",
    n_provider_error_title: "QuotaDock cannot refresh Codex usage",
    n_provider_error_body: "The last refresh attempts failed. QuotaDock keeps retrying.",
    n_also: "Also",
    n_test_title: "QuotaDock test notification",
    n_test_body: "Notifications are working.",
};

pub const ZH: Strings = Strings {
    app: "QuotaDock",
    paused_title: "QuotaDock — 已暂停",
    five_hour: "5小时",
    weekly: "每周",
    codex: "Codex",
    menu_open: "打开 QuotaDock",
    menu_refresh: "立即刷新",
    menu_pause: "暂停监控",
    menu_resume: "恢复监控",
    menu_quit: "退出",
    availability_unknown: "可用状态未知",
    availability_unknown_persistent: "可用状态持续未知",
    limit_reached: "已达到限制",
    not_signed_in: "请先登录 Codex",
    not_signed_in_short: "请先登录",
    codex_not_found: "未找到 Codex",
    provider_unavailable: "无法刷新 Codex 用量",
    provider_unavailable_short: "无法刷新",
    codex_unverified: "Codex 版本尚未验证",
    codex_unverified_short: "版本未验证",
    no_data_yet: "正在连接 Codex…",
    close_to_tray_title: "QuotaDock 仍在运行",
    close_to_tray_body: "QuotaDock 会继续在系统托盘中运行。",
    n_available_title: "Codex 已恢复可用",
    n_available_body: "Codex 报告普通用量已恢复可用。",
    n_credit_title: "Codex 重置机会已使用",
    n_credit_body: "检测到 Codex 额度变化，\n已使用 1 次重置机会。",
    n_refresh_title: "Codex 额度提前重置",
    n_allowance_title: "Codex 额度已重置",
    n_allowance_body: "上一个额度周期已结束。",
    n_low_combined_title: "Codex 额度较低",
    n_forecast_title: "Codex 额度可能提前用完",
    n_forecast_body: "QuotaDock 估算当前使用速度可能导致额度在下一次确认重置前用完。",
    n_provider_error_title: "QuotaDock 无法刷新 Codex 用量",
    n_provider_error_body: "最近几次刷新都失败了，QuotaDock 会继续重试。",
    n_also: "另外",
    n_test_title: "QuotaDock 测试通知",
    n_test_body: "通知功能正常。",
};

pub fn strings(lang: Lang) -> &'static Strings {
    match lang {
        Lang::En => &EN,
        Lang::ZhCn => &ZH,
    }
}

pub fn window_label(lang: Lang, five_hour: bool) -> &'static str {
    let s = strings(lang);
    if five_hour { s.five_hour } else { s.weekly }
}

/// "Codex weekly quota below 30%" / "Codex 每周额度低于 30%"
pub fn quota_below(lang: Lang, five_hour: bool, threshold: u32) -> String {
    quota_below_for(lang, "Codex", five_hour, threshold)
}

/// "Claude weekly quota below 30%" / "Claude 每周额度低于 30%"
pub fn quota_below_for(lang: Lang, provider: &str, five_hour: bool, threshold: u32) -> String {
    match lang {
        Lang::En => format!("{provider} {} quota below {threshold}%", if five_hour { "5-hour" } else { "weekly" }),
        Lang::ZhCn => format!("{provider} {}额度低于 {threshold}%", if five_hour { "5 小时" } else { "每周" }),
    }
}

/// "观察到 Codex 每周额度提前重置，具体原因未知。" Only the observation is reported: no promotion,
/// reset credit, server-confirmed cause or new billing window is ever inferred.
pub fn early_refresh_body(lang: Lang, five_hour: bool) -> String {
    match lang {
        Lang::En => format!(
            "QuotaDock observed an early reset of the Codex {} quota. The cause is unknown.",
            if five_hour { "5-hour" } else { "weekly" }
        ),
        Lang::ZhCn => format!("观察到 Codex {}额度提前重置，具体原因未知。", if five_hour { "5 小时" } else { "每周" }),
    }
}

/// "Codex quota is running low" / "Codex 额度较低"
pub fn low_combined_title(lang: Lang, provider: &str) -> String {
    match lang {
        Lang::En => format!("{provider} quota is running low"),
        Lang::ZhCn => format!("{provider} 额度较低"),
    }
}

/// Several providers low in one evaluation.
pub fn multi_low_title(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "AI quotas are running low",
        Lang::ZhCn => "AI 额度偏低",
    }
}

/// "Claude weekly: 9%" / "Claude 每周：9%" (remaining)
pub fn provider_window_remaining(lang: Lang, provider: &str, five_hour: bool, remaining: f64) -> String {
    let r = remaining.round() as i64;
    match lang {
        Lang::En => format!("{provider} {}: {r}% left", if five_hour { "5-hour" } else { "weekly" }),
        Lang::ZhCn => format!("{provider} {}：剩余 {r}%", if five_hour { "5 小时" } else { "每周" }),
    }
}

/// "Claude quota may run out early" / "Claude 额度可能提前用完"
pub fn forecast_title_for(lang: Lang, provider: &str) -> String {
    match lang {
        Lang::En => format!("{provider} quota may run out early"),
        Lang::ZhCn => format!("{provider} 额度可能提前用完"),
    }
}

/// A forecast folded into another notification's "Also" line still says it is an estimate.
pub fn forecast_short(lang: Lang, title: &str) -> String {
    match lang {
        Lang::En => format!("{title} (estimated)"),
        Lang::ZhCn => format!("{title}（估算）"),
    }
}

/// "Claude 5-hour quota reset" / "Claude 5 小时额度已重置"
pub fn allowance_refreshed_for(lang: Lang, provider: &str, window: Option<&str>) -> String {
    let w = |en: bool| match window {
        Some("five_hour") => {
            if en {
                "5-hour "
            } else {
                "5 小时"
            }
        }
        Some("weekly") => {
            if en {
                "weekly "
            } else {
                "每周"
            }
        }
        _ => "",
    };
    match lang {
        Lang::En => format!("{provider} {}quota reset", w(true)),
        Lang::ZhCn => format!("{provider} {}额度已重置", w(false)),
    }
}

pub fn claude_refresh_body(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "The previous reset time passed and Claude Code reported a new period. Cause not inferred.",
        Lang::ZhCn => "上一个重置时间已过，Claude Code 报告了新的额度周期。未推断原因。",
    }
}

/// Claude Code reported signed out after QuotaDock had seen it signed in (see `claude_auth`).
pub fn claude_signed_out_title(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Claude monitoring stopped",
        Lang::ZhCn => "Claude 监控已中断",
    }
}

pub fn claude_signed_out_body(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Claude Code is signed out. Sign in again to resume monitoring.",
        Lang::ZhCn => "Claude Code 已退出登录，请重新登录。",
    }
}

/// Title and body in one line, for when the notice is folded into another notification.
pub fn claude_signed_out_line(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Claude monitoring stopped: Claude Code is signed out. Sign in again to resume monitoring.",
        Lang::ZhCn => "Claude 监控已中断：Claude Code 已退出登录，请重新登录。",
    }
}

/// Tray menu/tooltip line for a provider whose data is not live: "Claude · Not live" / "Claude · 非实时"
pub fn not_live(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "Not live",
        Lang::ZhCn => "非实时",
    }
}

pub fn claude_state_short(lang: Lang, state: &str) -> &'static str {
    match (lang, state) {
        (Lang::En, "not_configured") => "Not set up",
        (Lang::ZhCn, "not_configured") => "未设置",
        (Lang::En, "waiting") => "Waiting for Claude Code",
        (Lang::ZhCn, "waiting") => "等待 Claude Code",
        (Lang::En, "api_billing") => "API billing",
        (Lang::ZhCn, "api_billing") => "API 计费",
        (Lang::En, "not_found") => "Not installed",
        (Lang::ZhCn, "not_found") => "未安装",
        (Lang::En, "not_authenticated") => "Not signed in",
        (Lang::ZhCn, "not_authenticated") => "未登录",
        (Lang::En, "paused") => "Paused",
        (Lang::ZhCn, "paused") => "已暂停",
        (Lang::En, "disabled") => "Off",
        (Lang::ZhCn, "disabled") => "已关闭",
        (Lang::En, "needs_verification") => "Needs verification",
        (Lang::ZhCn, "needs_verification") => "需要验证",
        (Lang::En, "source_error") => "Cannot read data",
        (Lang::ZhCn, "source_error") => "无法读取数据",
        (Lang::En, _) => "Unavailable",
        (Lang::ZhCn, _) => "不可用",
    }
}

/// `claude_state_short` where the full words do not fit: the widget's compact name slot, measured
/// beside both value columns. None: they fit, as all the Chinese ones do.
pub fn claude_state_compact(lang: Lang, state: &str) -> Option<&'static str> {
    match (lang, state) {
        (Lang::En, "needs_verification") => Some("Needs check"),
        (Lang::En, "source_error") => Some("Can't read"),
        _ => None,
    }
}

/// "9% weekly quota remaining." / "每周额度剩余 9%。"
pub fn remaining_line(lang: Lang, five_hour: bool, remaining: f64) -> String {
    let r = remaining.round() as i64;
    match lang {
        Lang::En => format!("{r}% {} quota remaining.", if five_hour { "5-hour" } else { "weekly" }),
        Lang::ZhCn => format!("{}额度剩余 {r}%。", if five_hour { "5 小时" } else { "每周" }),
    }
}

pub fn combined_low_body(lang: Lang, five_hour: bool, remaining: f64) -> String {
    match lang {
        Lang::En => format!(
            "{}\nQuotaDock estimates it may run out before the next confirmed reset.",
            remaining_line(lang, five_hour, remaining)
        ),
        Lang::ZhCn => {
            format!("{}\nQuotaDock 估算当前使用速度可能导致额度在下一次确认重置前用完。", remaining_line(lang, five_hour, remaining))
        }
    }
}

pub fn credits_line(lang: Lang, before: i64, after: i64) -> String {
    match lang {
        Lang::En => format!("{before} → {after} credits remaining."),
        Lang::ZhCn => format!("剩余重置机会：{before} → {after}。"),
    }
}

/// "Updated 3 min ago" / "3分钟前更新"
pub fn updated_ago(lang: Lang, minutes: i64) -> String {
    match (lang, minutes) {
        (Lang::En, m) if m < 1 => "Updated just now".into(),
        (Lang::En, m) => format!("Updated {m} min ago"),
        (Lang::ZhCn, m) if m < 1 => "刚刚更新".into(),
        (Lang::ZhCn, m) => format!("{m}分钟前更新"),
    }
}

pub fn maybe_stale(lang: Lang, minutes: i64) -> String {
    match lang {
        Lang::En => format!("Data may be stale · {minutes} min"),
        Lang::ZhCn => format!("数据可能已过时 · {minutes} 分钟"),
    }
}

/// An age the way the pages say it: minutes below two hours, then hours.
fn age_span(lang: Lang, minutes: i64) -> String {
    let hours = minutes / 60;
    match (lang, minutes < 120) {
        (Lang::En, true) => format!("{minutes} min"),
        (Lang::ZhCn, true) => format!("{minutes} 分钟"),
        (Lang::En, false) => format!("{hours} h"),
        (Lang::ZhCn, false) => format!("{hours} 小时"),
    }
}

pub fn stale(lang: Lang, minutes: i64) -> String {
    let age = age_span(lang, minutes);
    match lang {
        Lang::En => format!("Data stale · {age}"),
        Lang::ZhCn => format!("数据已过时 · {age}"),
    }
}

pub fn last_update(lang: Lang, minutes: i64) -> String {
    let age = age_span(lang, minutes);
    match (lang, minutes < 1) {
        (Lang::En, true) => "Last update: just now".into(),
        (Lang::ZhCn, true) => "上次更新：刚刚".into(),
        (Lang::En, false) => format!("Last update: {age} ago"),
        (Lang::ZhCn, false) => format!("上次更新：{age}前"),
    }
}

pub fn percent_line(lang: Lang, five_hour: Option<f64>, weekly: Option<f64>) -> String {
    let f = |v: Option<f64>| v.map(|x| format!("{}%", x.round() as i64)).unwrap_or_else(|| "—".into());
    match lang {
        Lang::En => format!("5h: {} · Weekly: {}", f(five_hour), f(weekly)),
        Lang::ZhCn => format!("5小时：{} · 每周：{}", f(five_hour), f(weekly)),
    }
}

/// `percent_line` for a provider whose 5-hour window is hidden: the weekly value only.
pub fn weekly_line(lang: Lang, weekly: Option<f64>) -> String {
    let w = weekly.map(|x| format!("{}%", x.round() as i64)).unwrap_or_else(|| "—".into());
    match lang {
        Lang::En => format!("Weekly: {w}"),
        Lang::ZhCn => format!("每周：{w}"),
    }
}

/// The desktop widget's right-click menu (Phase 3.7).
pub struct WidgetMenu {
    pub open: &'static str,
    pub lock: &'static str,
    pub on_top: &'static str,
    pub detailed: &'static str,
    pub hide: &'static str,
}

pub fn widget_menu(lang: Lang) -> WidgetMenu {
    match lang {
        Lang::En => WidgetMenu {
            open: "Open QuotaDock",
            lock: "Lock position",
            on_top: "Always on top",
            detailed: "Detailed view",
            hide: "Hide widget",
        },
        Lang::ZhCn => WidgetMenu { open: "打开 QuotaDock", lock: "锁定位置", on_top: "总在最前", detailed: "详细模式", hide: "隐藏小组件" },
    }
}