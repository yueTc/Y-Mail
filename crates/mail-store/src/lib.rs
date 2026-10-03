//! mail-store：本地存储层，SQLite 的唯一写入口。
//!
//! 约定：其它 crate 不得直接访问数据库，只能经本 crate 的接口读写。
//! Wave 0 实现连接（WAL）与迁移机制；业务表自 Wave 1 起逐步添加。

pub mod connection;
pub mod error;
pub mod migrations;

pub use connection::Store;
pub use error::StoreError;
pub use migrations::{MigrationOutcome, MigrationReport};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SQLite 存储与迁移（唯一写库者）";
