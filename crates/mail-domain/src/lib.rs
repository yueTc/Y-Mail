//! mail-domain：领域模型与共享类型。
//!
//! 本 crate 是依赖图的根节点：不依赖任何其它 crate，不做 I/O，不认识数据库与网络。
//! 其它所有 crate 都可以依赖它；它不依赖任何人。
//!
//! Wave 0 仅提供骨架；领域类型（账号、文件夹、邮件、线程）自 Wave 1 起逐步填充。

/// 本 crate 的用途标识，供工作区自检与日志使用。
pub const CRATE_PURPOSE: &str = "领域模型与共享类型（无 I/O）";

#[cfg(test)]
mod tests {
    use super::CRATE_PURPOSE;

    #[test]
    fn purpose_is_not_empty() {
        assert!(!CRATE_PURPOSE.is_empty());
    }
}
