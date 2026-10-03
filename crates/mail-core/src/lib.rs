//! mail-core：引擎门面（facade）。
//!
//! 职责：对桌面外壳与外部接入只暴露干净接口，内部编排存储与协议层；
//! 未来可整体抽为独立后台进程（daemon），UI 层无需改写。
//!
//! Wave 0 仅提供骨架与初始化入口；具体业务自 Wave 1 起填充。

pub mod engine;
pub mod paths;

pub use engine::{EngineInit, MailEngine};
pub use paths::SqlitePaths;

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "引擎门面（唯一对外接口）";
