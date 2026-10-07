//! 引擎使用的本地路径约定。
//!
//! 只处理路径拼装，不创建目录、不打开文件——落盘动作统一由 `MailEngine` 在初始化时执行。

use std::path::{Path, PathBuf};

/// 引擎数据目录下的文件布局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlitePaths {
    /// 数据根目录（由外壳传入，例如 Windows 的 `%APPDATA%/<应用标识>`）。
    pub root_dir: PathBuf,
    /// 主数据库文件。
    pub database_file: PathBuf,
    /// 日志目录。
    pub log_dir: PathBuf,
    /// 附件下载目录；可以在设置里单独指定，默认落在数据根目录下的 `downloads`。
    pub attachment_dir: PathBuf,
}

impl SqlitePaths {
    /// 按约定的文件名，从数据根目录推导全部路径。
    pub fn from_root(root_dir: impl Into<PathBuf>) -> Self {
        Self::from_root_with_attachment(root_dir, None)
    }

    /// 同 [`SqlitePaths::from_root`]，但附件目录可以单独指定。
    ///
    /// 传 `None` 表示沿用默认的「数据根目录 / downloads」。
    pub fn from_root_with_attachment(root_dir: impl Into<PathBuf>, attachment_dir: Option<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        let attachment_dir = attachment_dir.unwrap_or_else(|| root_dir.join("downloads"));
        Self {
            database_file: root_dir.join("ymail.db"),
            log_dir: root_dir.join("logs"),
            attachment_dir,
            root_dir,
        }
    }

    /// 数据根目录。
    pub fn root(&self) -> &Path {
        &self.root_dir
    }
}

#[cfg(test)]
mod tests {
    use super::SqlitePaths;

    #[test]
    fn paths_are_derived_from_root() {
        let paths = SqlitePaths::from_root("C:/data/ymail");
        assert_eq!(
            paths.database_file,
            std::path::PathBuf::from("C:/data/ymail/ymail.db")
        );
        assert_eq!(paths.log_dir, std::path::PathBuf::from("C:/data/ymail/logs"));
        assert_eq!(
            paths.attachment_dir,
            std::path::PathBuf::from("C:/data/ymail/downloads")
        );
    }

    #[test]
    fn attachment_dir_can_be_overridden() {
        let paths = SqlitePaths::from_root_with_attachment(
            "C:/data/ymail",
            Some(std::path::PathBuf::from("D:/MailFiles")),
        );
        assert_eq!(paths.attachment_dir, std::path::PathBuf::from("D:/MailFiles"));
        assert_eq!(paths.root(), std::path::Path::new("C:/data/ymail"));
    }
}
