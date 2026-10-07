//! 让 cargo 知道：这两个环境变量变了就得重新编译本 crate。
//!
//! 否则用 option_env! 写进程序的内置登录编号会一直是旧值。

fn main() {
    println!("cargo:rerun-if-env-changed=YMAIL_GMAIL_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=YMAIL_MICROSOFT_CLIENT_ID");
}
