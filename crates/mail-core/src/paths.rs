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
}

impl SqlitePaths {
    /// 按约定的文件名，从数据根目录推导全部路径。
    pub fn from_root(root_dir: impl Into<PathBuf>) -> Self {
        let root_dir = root_dir.into();
        Self {
            database_file: root_dir.join("em-master.db"),
            log_dir: root_dir.join("logs"),
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
        let paths = SqlitePaths::from_root("C:/data/em-master");
        assert_eq!(
            paths.database_file,
            std::path::PathBuf::from("C:/data/em-master/em-master.db")
        );
        assert_eq!(paths.log_dir, std::path::PathBuf::from("C:/data/em-master/logs"));
    }
}
