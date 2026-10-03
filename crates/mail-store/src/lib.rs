//! mail-store：本地存储层，SQLite 的唯一写入口。
//!
//! 约定：其它 crate 不得直接访问数据库，只能经本 crate 的接口读写。
//! Wave 0 实现连接（WAL）与迁移机制；Wave 1 加入账号与代理表；
//! Wave 2 加入文件夹、邮件与同步任务表。

pub mod accounts;
pub mod connection;
pub mod error;
pub mod migrations;
pub mod proxies;
pub mod sync;

pub use connection::Store;
pub use error::StoreError;
pub use migrations::{MigrationOutcome, MigrationReport};
pub use proxies::StoredProxy;
pub use sync::{NewMessage, StoredFolder, StoredSyncJob};

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "SQLite 存储与迁移（唯一写库者）";
