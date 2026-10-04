//! `em-master-mcp`：MCP stdio 服务端可执行入口。
//!
//! 由外部 Agent（Codex / Claude Desktop / Cursor 等）以本地进程方式拉起；
//! 只通过标准输入输出通信，**不监听任何网络端口**。
//!
//! 启动顺序：解析数据目录 → 打开本地引擎 → 查 MCP 总开关 → 默认关闭就直接退出。

use std::process::ExitCode;

fn main() -> ExitCode {
    let data_dir = match mail_mcp::resolve_data_dir() {
        Ok(dir) => dir,
        Err(message) => {
            eprintln!("em-master-mcp：{message}");
            return ExitCode::from(2);
        }
    };

    let engine = match mail_core::MailEngine::initialize(&data_dir) {
        Ok(engine) => engine,
        Err(error) => {
            eprintln!("em-master-mcp：初始化本地数据失败：{error}");
            return ExitCode::from(2);
        }
    };

    // 默认关闭：没启用就直接退出，外部 Agent 连不上（Scenario 12.1）。
    match engine.mcp_status() {
        Ok(status) if status.enabled => {}
        Ok(_) => {
            eprintln!(
                "em-master-mcp：MCP 未启用。请在应用的「设置 → MCP 外部接入」里打开总开关后再启动本进程。"
            );
            return ExitCode::from(3);
        }
        Err(error) => {
            eprintln!("em-master-mcp：读取 MCP 开关失败：{error}");
            return ExitCode::from(2);
        }
    }

    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("em-master-mcp：启动运行时失败：{error}");
            return ExitCode::from(2);
        }
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match mail_mcp::serve(engine, stdin.lock(), stdout.lock(), &runtime) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("em-master-mcp：标准输入输出出错：{error}");
            ExitCode::from(2)
        }
    }
}
