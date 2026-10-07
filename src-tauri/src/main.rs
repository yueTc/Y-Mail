//! 桌面程序入口。
//!
//! 只做一件事：调用库里的 `run()`。真正的逻辑都在 `lib.rs`，
//! 这样将来若要加移动端或测试入口，也不必改动这里。

// 发布版不要弹出黑色的控制台窗口；调试版保留控制台，方便看日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    ymail_lib::run();
}
