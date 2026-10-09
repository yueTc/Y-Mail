//! GitHub 登录与设置同步命令（Wave S6）。
//!
//! 职责边界：外壳只做参数校验与转发，加密、出网、落盘都在 `mail-core` / `mail-sync`。
//! 安全约定：
//! - 命令入参里的同步密码、令牌一律不写日志；
//! - 出参只含公开状态（登录名 / 昵称 / 头像地址 / 版本号 / 错误提示），不含任何密钥；
//! - 头像地址由前端限定在固定域名加载，这里只如实透传。

use mail_core::{
    ConflictChoice, GitHubDeviceLoginView, GitHubLoginPoll, GitHubLoginView, GitHubScope, SettingsSyncStatus,
};

use crate::commands::CommandError;
use crate::state::AppState;

/// 同步密码最短长度（与后端保持一致）。
const MIN_PASSWORD_CHARS: usize = 8;

/// 校验「设置同步」的新密码：至少 8 位、两次一致。
fn validate_new_password(password: &str, confirm: &str) -> Result<(), CommandError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(CommandError::input(format!(
            "同步密码至少 {MIN_PASSWORD_CHARS} 位"
        )));
    }
    if password != confirm {
        return Err(CommandError::input("两次输入的同步密码不一致"));
    }
    Ok(())
}

/// 校验设备名非空（避免同步状态里出现一台没名字的设备）。
fn validate_device_label(label: &str) -> Result<(), CommandError> {
    if label.trim().is_empty() {
        return Err(CommandError::input("请填写这台设备的名字"));
    }
    Ok(())
}

/// 发起一次 GitHub 设备码登录；`scope` 传 `sync` 时多要 Gist 权限。
#[tauri::command]
pub async fn github_login_start(
    state: tauri::State<'_, AppState>,
    scope: Option<String>,
) -> Result<GitHubDeviceLoginView, CommandError> {
    let scope = match scope.as_deref() {
        Some("sync") => GitHubScope::Sync,
        _ => GitHubScope::Login,
    };
    let engine = state.engine().await;
    Ok(engine.start_github_login(scope).await?)
}

/// 轮询一次设备码登录结果。
#[tauri::command]
pub async fn github_login_poll(
    state: tauri::State<'_, AppState>,
    login_id: String,
) -> Result<GitHubLoginPoll, CommandError> {
    if login_id.trim().is_empty() {
        return Err(CommandError::input("登录编号为空"));
    }
    let engine = state.engine().await;
    Ok(engine.poll_github_login(login_id.trim()).await?)
}

/// 退出 GitHub 登录：清保险箱令牌与库里资料。
#[tauri::command]
pub async fn github_login_sign_out(state: tauri::State<'_, AppState>) -> Result<(), CommandError> {
    let engine = state.engine().await;
    engine.sign_out_github()?;
    Ok(())
}

/// 读本机已登录的 GitHub 资料；没登录返回 `null`。
#[tauri::command]
pub async fn github_login_profile(
    state: tauri::State<'_, AppState>,
) -> Result<Option<GitHubLoginView>, CommandError> {
    let engine = state.engine().await;
    Ok(engine.github_login()?)
}

/// 当前设置同步状态。
#[tauri::command]
pub async fn settings_sync_status(
    state: tauri::State<'_, AppState>,
) -> Result<SettingsSyncStatus, CommandError> {
    let engine = state.engine().await;
    Ok(engine.settings_sync_status()?)
}

/// 开启设置同步：设密码、把本机配置加密后建一个私密 Gist。
#[tauri::command]
pub async fn settings_sync_enable(
    state: tauri::State<'_, AppState>,
    password: String,
    confirm: String,
    device_label: String,
) -> Result<SettingsSyncStatus, CommandError> {
    validate_new_password(&password, &confirm)?;
    validate_device_label(&device_label)?;
    let engine = state.engine().await;
    Ok(engine
        .enable_settings_sync(&password, &confirm, &device_label)
        .await?)
}

/// 关闭设置同步；`delete_remote` 为真时连云端那份一起删。
#[tauri::command]
pub async fn settings_sync_disable(
    state: tauri::State<'_, AppState>,
    delete_remote: Option<bool>,
) -> Result<SettingsSyncStatus, CommandError> {
    let engine = state.engine().await;
    Ok(engine
        .disable_settings_sync(delete_remote.unwrap_or(false))
        .await?)
}

/// 立即同步：先拉远端比对，没有冲突就把本机改动传上去。
#[tauri::command]
pub async fn settings_sync_now(
    state: tauri::State<'_, AppState>,
) -> Result<SettingsSyncStatus, CommandError> {
    let engine = state.engine().await;
    Ok(engine.sync_now().await?)
}

/// 重设同步密码：建新 Gist、删旧 Gist。
#[tauri::command]
pub async fn settings_sync_reset_password(
    state: tauri::State<'_, AppState>,
    new_password: String,
    confirm: String,
) -> Result<SettingsSyncStatus, CommandError> {
    validate_new_password(&new_password, &confirm)?;
    let engine = state.engine().await;
    Ok(engine
        .reset_settings_sync_password(&new_password, &confirm)
        .await?)
}

/// 新设备加入：输同步密码解密云端配置并导入。
#[tauri::command]
pub async fn settings_sync_join(
    state: tauri::State<'_, AppState>,
    password: String,
    device_label: String,
    confirm_overwrite: Option<bool>,
) -> Result<SettingsSyncStatus, CommandError> {
    if password.is_empty() {
        return Err(CommandError::input("请填写同步密码"));
    }
    validate_device_label(&device_label)?;
    let engine = state.engine().await;
    Ok(engine
        .join_settings_sync(&password, &device_label, confirm_overwrite.unwrap_or(false))
        .await?)
}

/// 解决冲突：保留本机重新上传，或丢本机按远端导入。
#[tauri::command]
pub async fn settings_sync_resolve_conflict(
    state: tauri::State<'_, AppState>,
    choice: ConflictChoice,
) -> Result<SettingsSyncStatus, CommandError> {
    let engine = state.engine().await;
    Ok(engine.resolve_settings_sync_conflict(choice).await?)
}
