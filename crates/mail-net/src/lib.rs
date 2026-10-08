//! 共享网络连接层：把「选代理 → 穿代理建连 → 套加密」收敛在一个 crate 里，
//! 供收件自检与发件自检共用，避免两处各写一遍。
//!
//! 依赖方向：mail-domain ← mail-net ← mail-imap / mail-smtp ← mail-core。
//! 本 crate 只负责传输，不认识任何邮件协议。

pub mod connect;
pub mod error;
pub mod io;
pub mod probe;
pub mod stream;
pub mod system;

/// 本 crate 的存在目的，供检查脚本读取。
pub const CRATE_PURPOSE: &str = "共享网络连接层：直连、代理隧道与 TLS 包装";

pub use connect::{connect_tcp, DEFAULT_TIMEOUT};
pub use probe::probe_website;
pub use stream::{tls_wrap, Stream};
pub use system::{parse_proxy_server, read_system_proxy, SystemProxy};
