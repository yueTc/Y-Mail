//! MCP（外部 Agent 接入）命令（Wave 8）。
//!
//! 职责边界：外壳只做转发；开关与审计都在 `mail-core` / `mail-store`。
//! 安全约定：MCP 是本地 stdio 进程，应用侧不监听任何端口；
//! 命令出参只含开关、工具清单与审计（审计只有参数哈希，没有正文），不回凭据任何内容。

use mail_core::MailEngine;
use serde::Serialize;

use crate::commands::CommandError;
use crate::state::AppState;

/// MCP 服务端可执行文件名（安装包自带，放在应用安装目录）。
const MCP_BINARY_NAME: &str = "em-master-mcp";
/// 外部 Agent 配置里用的数据目录环境变量名（与 mail-mcp 保持同一约定）。
const MCP_DATA_DIR_ENV: &str = "EM_MASTER_DATA_DIR";

/// 当前 MCP 状态与配置说明（给设置页显示）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStatusDto {
    /// 总开关；默认 false。
    pub enabled: bool,
    /// 写工具（建草稿）独立开关；默认 false。
    pub write_tools_enabled: bool,
    /// 本地数据目录（数据库所在目录）。
    pub data_dir: String,
    /// 服务端可执行文件名。
    pub binary_name: String,
    /// 安装目录里服务端可执行文件的完整路径（安装包自带）。
    pub binary_path: String,
    /// 配置里要填的数据目录环境变量名。
    pub data_dir_env: String,
    /// 支持的协议版本。
    pub protocol_versions: Vec<String>,
    /// 给外部 Agent 的配置示例（JSON 文本）。
    pub config_example: String,
}

/// 工具清单里的一条。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDto {
    /// 工具名。
    pub name: String,
    /// 中文短标题。
    pub title: String,
    /// 说明。
    pub description: String,
    /// 是否只读。
    pub read_only: bool,
    /// 当前开关下是否可用。
    pub enabled: bool,
}

/// 一条审计记录；不含正文、不含凭据。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuditDto {
    /// 主键。
    pub id: i64,
    /// 工具名。
    pub tool: String,
    /// 账号范围。
    pub account_scope: String,
    /// 参数摘要（哈希）。
    pub args_digest: String,
    /// 状态。
    pub status: String,
    /// 时间。
    pub ts: String,
}

/// 审计列表。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpAuditPageDto {
    /// 记录（时间倒序）。
    pub items: Vec<McpAuditDto>,
    /// 总条数。
    pub total: i64,
}

/// 服务端可执行文件名（Windows 带 .exe，其它平台不带）。
fn sidecar_file_name() -> String {
    if cfg!(windows) {
        format!("{MCP_BINARY_NAME}.exe")
    } else {
        MCP_BINARY_NAME.to_string()
    }
}

/// 服务端可执行文件是否已随安装包安装到应用安装目录。
fn sidecar_installed() -> bool {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
    else {
        return false;
    };
    dir.join(sidecar_file_name()).is_file()
}

/// 组装状态摘要。
fn status_dto(engine: &MailEngine) -> Result<McpStatusDto, CommandError> {
    let status = engine.mcp_status()?;
    let data_dir = engine.init_summary().root_dir;
    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|path| path.to_string_lossy().to_string()));
    let binary_path = match &install_dir {
        Some(dir) => std::path::Path::new(dir)
            .join(sidecar_file_name())
            .to_string_lossy()
            .to_string(),
        None => format!("<应用安装目录>\\{}", sidecar_file_name()),
    };
    let example = config_example(&binary_path, &data_dir);
    Ok(McpStatusDto {
        enabled: status.enabled,
        write_tools_enabled: status.write_tools_enabled,
        data_dir: data_dir.clone(),
        binary_name: MCP_BINARY_NAME.to_string(),
        binary_path,
        data_dir_env: MCP_DATA_DIR_ENV.to_string(),
        protocol_versions: mail_mcp::SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .map(|value| value.to_string())
            .collect(),
        config_example: example,
    })
}

/// 生成外部 Agent 的配置示例。
///
/// 安装包把 `em-master-mcp.exe` 与主程序放在同一个安装目录，所以 `command` 直接写
/// 安装目录里的完整路径（从本机正在运行的主程序位置推导），不用用户手填。
fn config_example(binary_path: &str, data_dir: &str) -> String {
    let example = serde_json::json!({
        "mcpServers": {
            "em-master": {
                "command": binary_path,
                "env": { MCP_DATA_DIR_ENV: data_dir }
            }
        }
    });
    let mut text = serde_json::to_string_pretty(&example).unwrap_or_else(|_| "{}".to_string());
    if !sidecar_installed() {
        text.push_str(
            "\n\n注意：当前运行的目录里还没找到 em-master-mcp.exe。\
             如果你是从源码直接跑（开发模式），请先执行 npm run build:mcp-sidecar 生成它，\
             或改用安装包安装后的版本。",
        );
    }
    text
}

/// 查询 MCP 状态与配置说明。
#[tauri::command]
pub async fn mcp_status(state: tauri::State<'_, AppState>) -> Result<McpStatusDto, CommandError> {
    let engine = state.engine().await;
    status_dto(&engine)
}

/// 一键启用 / 关闭 MCP。
#[tauri::command]
pub async fn mcp_set_enabled(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<McpStatusDto, CommandError> {
    let engine = state.engine().await;
    engine.mcp_set_enabled(enabled)?;
    status_dto(&engine)
}

/// 打开 / 关闭写工具（建草稿）开关。
#[tauri::command]
pub async fn mcp_set_write_tools(
    state: tauri::State<'_, AppState>,
    enabled: bool,
) -> Result<McpStatusDto, CommandError> {
    let engine = state.engine().await;
    engine.mcp_set_write_tools_enabled(enabled)?;
    status_dto(&engine)
}

/// 工具清单（只读工具 + 写工具及其当前是否可用）。
#[tauri::command]
pub async fn mcp_tools(state: tauri::State<'_, AppState>) -> Result<Vec<McpToolDto>, CommandError> {
    let engine = state.engine().await;
    let tools = engine.mcp_tools()?;
    Ok(tools
        .into_iter()
        .map(|tool| McpToolDto {
            name: tool.name.to_string(),
            title: tool.title.to_string(),
            description: tool.description.to_string(),
            read_only: tool.read_only,
            enabled: tool.enabled,
        })
        .collect())
}

/// 查 MCP 审计（时间倒序，最多 500 条）。
#[tauri::command]
pub async fn mcp_audit(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
) -> Result<McpAuditPageDto, CommandError> {
    let limit = limit.unwrap_or(100);
    let engine = state.engine().await;
    let items = engine.mcp_audit(limit)?;
    let total = engine.mcp_audit_count()?;
    Ok(McpAuditPageDto {
        items: items
            .into_iter()
            .map(|row| McpAuditDto {
                id: row.id,
                tool: row.tool,
                account_scope: row.account_scope,
                args_digest: row.args_digest,
                status: row.status,
                ts: row.ts,
            })
            .collect(),
        total,
    })
}
