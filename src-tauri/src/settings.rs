//! 应用设置：邮件数据目录、兼容附件目录字段、新邮件通知开关。
//!
//! 为什么放在固定位置：数据目录决定引擎在哪里开库，必须在引擎初始化之前读到，
//! 所以不能存进数据库。配置文件固定放在默认应用数据目录下的 `settings.json`，
//! 改过邮件目录之后下次启动仍然找得到这份配置。
//!
//! 安全约定：这里只记路径和布尔开关，不涉及任何凭据。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::storage_dir::DATABASE_NAME;

/// 设置文件名。
pub const SETTINGS_FILE: &str = "settings.json";

/// 自动检测更新的间隔允许范围（小时）与默认值。
pub const UPDATE_CHECK_INTERVAL_MIN_HOURS: u32 = 1;
pub const UPDATE_CHECK_INTERVAL_MAX_HOURS: u32 = 168;
pub const UPDATE_CHECK_INTERVAL_DEFAULT_HOURS: u32 = 24;

/// 存进 `settings.json` 的内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    /// 邮件数据目录（数据库与日志所在）；`None` 表示用默认应用数据目录。
    pub data_dir: Option<PathBuf>,
    /// 旧版单独附件目录；只保留读兼容，当前生效值固定为「数据目录 / downloads」。
    pub attachment_dir: Option<PathBuf>,
    /// 新邮件是否弹系统通知；默认开。
    pub notify_new_mail: bool,
    /// 通知智能识别（高级功能）：开启后新邮件正文会自动发往所选 AI；默认关。
    pub notify_ai_enabled: bool,
    /// 是不是默认拦截邮件里的远程图片；默认拦（true）。
    /// 关掉之后，读信时远程图片会直接放行；本封放行与「信任发件人」仍各自有效。
    pub block_remote_images_by_default: bool,
    /// 按关闭键时是收进托盘还是退出应用；默认收（true），保持一直以来的行为。
    pub minimize_to_tray_on_close: bool,
    /// 启动时是不是直接进托盘、不弹主窗口；默认否，双击图标仍能看到窗口。
    /// 被开机自启动拉起来的那一次不受这里影响，始终静默进托盘。
    pub start_minimized_to_tray: bool,
    /// 是否自动检测更新；默认关，打开后才会定期联网查有没有新版本。
    pub auto_check_update: bool,
    /// 自动检测更新的间隔小时数；默认 24，允许 1–168，越界读回时按默认处理。
    pub update_check_interval_hours: u32,
    /// 下次启动要清理的旧数据目录；为空表示没有待清理目录。
    pub pending_cleanup_dir: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            data_dir: None,
            attachment_dir: None,
            notify_new_mail: true,
            notify_ai_enabled: false,
            block_remote_images_by_default: true,
            minimize_to_tray_on_close: true,
            start_minimized_to_tray: false,
            auto_check_update: false,
            update_check_interval_hours: UPDATE_CHECK_INTERVAL_DEFAULT_HOURS,
            pending_cleanup_dir: None,
        }
    }
}

impl AppSettings {
    /// 从默认应用数据目录读配置；文件不存在或读坏都退回默认值，绝不拦启动。
    pub fn load(base_dir: &Path) -> Self {
        let path = base_dir.join(SETTINGS_FILE);
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match serde_json::from_str::<AppSettings>(&raw) {
            Ok(settings) => settings.normalize_interval(),
            Err(error) => {
                tracing::warn!(error = %error, path = %path.display(), "设置文件解析失败，改用默认设置");
                Self::default()
            }
        }
    }

    /// 把配置写回默认应用数据目录；先建目录再写。
    pub fn save(&self, base_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(base_dir)?;
        let raw = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(base_dir.join(SETTINGS_FILE), raw)
    }

    /// 把越界的检测间隔拉回默认值；老配置缺字段或手改坏了都不能拦启动。
    fn normalize_interval(mut self) -> Self {
        if !(UPDATE_CHECK_INTERVAL_MIN_HOURS..=UPDATE_CHECK_INTERVAL_MAX_HOURS)
            .contains(&self.update_check_interval_hours)
        {
            self.update_check_interval_hours = UPDATE_CHECK_INTERVAL_DEFAULT_HOURS;
        }
        self
    }

    /// 邮件数据目录的生效值：没单独配置就用传入的默认目录。
    pub fn effective_data_dir(&self, default_dir: &Path) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(|| default_dir.to_path_buf())
    }

    /// 下载目录的生效值：固定为「邮件数据目录 / downloads」。
    ///
    /// `attachment_dir` 字段仍能读旧配置，但保存设置和引擎启动都不再使用它。
    pub fn effective_attachment_dir(&self, default_dir: &Path) -> PathBuf {
        self.effective_data_dir(default_dir).join("downloads")
    }
}

/// 判断是不是全新安装后的第一次启动。
///
/// 依据是默认应用数据目录里「既没有 `settings.json`、也没有数据库文件」：
/// 老用户升级时这两样至少有一个在，不会被再问一次。
/// 必须在引擎建库之前调用——引擎一启动就会在默认目录里建库。
pub fn is_first_run(base_dir: &Path) -> bool {
    !base_dir.join(SETTINGS_FILE).exists() && !base_dir.join(DATABASE_NAME).exists()
}

#[cfg(test)]
mod tests {
    use super::{AppSettings, UPDATE_CHECK_INTERVAL_DEFAULT_HOURS};
    #[test]
    fn defaults_keep_current_behaviour() {
        let settings = AppSettings::default();
        assert_eq!(settings.data_dir, None);
        assert_eq!(settings.attachment_dir, None);
        assert!(settings.notify_new_mail);
        assert!(!settings.notify_ai_enabled);
        assert!(settings.block_remote_images_by_default);
        assert!(settings.minimize_to_tray_on_close, "默认按关闭键收进托盘");
        assert!(!settings.start_minimized_to_tray, "默认启动时显示主窗口");
        assert!(!settings.auto_check_update, "默认不自动检测更新");
        assert_eq!(settings.update_check_interval_hours, 24, "默认每 24 小时查一次");
        assert_eq!(settings.pending_cleanup_dir, None);
        assert_eq!(
            settings.effective_data_dir(std::path::Path::new("C:/app")),
            std::path::PathBuf::from("C:/app")
        );
        assert_eq!(
            settings.effective_attachment_dir(std::path::Path::new("C:/app")),
            std::path::PathBuf::from("C:/app/downloads")
        );
    }

    #[test]
    fn custom_data_dir_wins_and_legacy_attachment_dir_is_ignored() {
        let settings = AppSettings {
            data_dir: Some(std::path::PathBuf::from("D:/Mail")),
            attachment_dir: Some(std::path::PathBuf::from("E:/OldFiles")),
            notify_new_mail: false,
            notify_ai_enabled: false,
            block_remote_images_by_default: true,
            minimize_to_tray_on_close: true,
            start_minimized_to_tray: false,
            auto_check_update: false,
            update_check_interval_hours: UPDATE_CHECK_INTERVAL_DEFAULT_HOURS,
            pending_cleanup_dir: None,
        };
        assert_eq!(
            settings.effective_data_dir(std::path::Path::new("C:/app")),
            std::path::PathBuf::from("D:/Mail")
        );
        assert_eq!(
            settings.effective_attachment_dir(std::path::Path::new("C:/app")),
            std::path::PathBuf::from("D:/Mail/downloads")
        );
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "ymail-settings-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let settings = AppSettings {
            data_dir: Some(std::path::PathBuf::from("D:/Mail")),
            attachment_dir: Some(std::path::PathBuf::from("E:/Files")),
            notify_new_mail: false,
            notify_ai_enabled: true,
            block_remote_images_by_default: false,
            minimize_to_tray_on_close: false,
            start_minimized_to_tray: true,
            auto_check_update: true,
            update_check_interval_hours: 6,
            pending_cleanup_dir: Some(std::path::PathBuf::from("C:/Old/Mail")),
        };
        settings.save(&dir).expect("保存设置");
        let raw = std::fs::read_to_string(dir.join(super::SETTINGS_FILE)).expect("读设置原文");
        assert!(
            raw.contains("pendingCleanupDir"),
            "待清理字段要按 camelCase 落盘：{raw}"
        );
        let loaded = AppSettings::load(&dir);
        assert_eq!(loaded, settings);
        // 缺失的字段走默认：只写一个开关也能读出来。
        std::fs::write(dir.join(super::SETTINGS_FILE), "{ \"notifyNewMail\": false }").expect("写文件");
        let partial = AppSettings::load(&dir);
        assert!(!partial.notify_new_mail);
        assert!(
            partial.block_remote_images_by_default,
            "老配置里缺这个字段时要按默认拦截处理"
        );
        assert!(!partial.notify_ai_enabled, "老配置里缺通知识别开关时要按关闭处理");
        assert!(
            partial.minimize_to_tray_on_close,
            "老配置里缺这个字段时按「关闭收托盘」处理"
        );
        assert!(
            !partial.start_minimized_to_tray,
            "老配置里缺这个字段时按「启动不静默」处理"
        );
        assert!(!partial.auto_check_update, "老配置缺自动检测开关时按关闭处理");
        assert_eq!(
            partial.update_check_interval_hours, 24,
            "老配置缺检测间隔时按 24 小时"
        );
        assert_eq!(partial.data_dir, None);
        assert_eq!(partial.attachment_dir, None);
        assert_eq!(partial.pending_cleanup_dir, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn out_of_range_update_interval_falls_back_to_default() {
        let dir = std::env::temp_dir().join(format!(
            "ymail-settings-interval-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("建目录");
        std::fs::write(
            dir.join(super::SETTINGS_FILE),
            "{ \"autoCheckUpdate\": true, \"updateCheckIntervalHours\": 999 }",
        )
        .expect("写文件");
        let loaded = AppSettings::load(&dir);
        assert!(loaded.auto_check_update);
        assert_eq!(loaded.update_check_interval_hours, 24);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_run_needs_both_settings_and_database_absent() {
        let root = tempfile::tempdir().expect("临时目录");
        let dir = root.path();
        assert!(super::is_first_run(dir), "空目录应判定为首次启动");
        std::fs::write(dir.join(super::SETTINGS_FILE), "{}").expect("写设置");
        assert!(!super::is_first_run(dir), "有设置文件就不算首次启动");
        std::fs::remove_file(dir.join(super::SETTINGS_FILE)).expect("删设置");
        std::fs::write(dir.join(crate::storage_dir::DATABASE_NAME), b"").expect("写数据库占位");
        assert!(!super::is_first_run(dir), "有数据库就不算首次启动");
    }
}
