//! 邮箱文件夹管理与单封邮件删除。
//!
//! 设计口径（规格 `2026-10-10-mail-folder-management-design.md`）：
//! - 所有写操作先落到服务商，服务商确认后才改本地库，不出现假成功；
//! - 系统文件夹与协议不允许的文件夹在界面上不开放入口，这里再兜一层；
//! - 删除文件夹只允许删空的（没邮件、没子文件夹），不自动清空、不递归。

use std::sync::Mutex;

use mail_domain::{folder_allows_children, folder_changeable, ConnectionErrorKind, FolderKind};
use mail_imap::{build_server_path, rejection_blocks_children, FolderInfo};
use mail_store::{NewFolder, Store};

use crate::compose::lock_store;
use crate::engine::{EngineError, MailEngine};

/// 删除一封邮件前的预览：告诉界面该弹哪种确认框。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteMessagePreview {
    /// 移到「已删除」时的目标文件夹名；彻底删除时为 None。
    pub target_folder: Option<String>,
    /// 是否会被彻底删除（界面必须再确认一次）。
    pub permanent: bool,
}

/// 把服务器刚返回的文件夹列表对齐到本地库。
fn align_server_folders(
    store: &Mutex<Store>,
    account_id: i64,
    folders: &[FolderInfo],
) -> Result<(), EngineError> {
    let prepared: Vec<NewFolder> = folders
        .iter()
        .map(|info| NewFolder {
            full_path: info.full_path.clone(),
            server_path: info.server_path.clone(),
            delimiter: info.delimiter.clone(),
            kind: FolderKind::classify(&info.full_path, &info.attributes),
            attributes: info.attributes.clone(),
        })
        .collect();
    let store = lock_store(store);
    store.align_folders(account_id, &prepared)?;
    Ok(())
}

/// 按分隔符拆出父路径和最后一段名字。
fn split_parent(path: &str, delimiter: &str) -> (Option<String>, String) {
    if delimiter.is_empty() {
        return (None, path.to_string());
    }
    match path.rfind(delimiter) {
        Some(index) => (
            Some(path[..index].to_string()),
            path[index + delimiter.len()..].to_string(),
        ),
        None => (None, path.to_string()),
    }
}

impl MailEngine {
    /// 在指定账号下新建文件夹。
    ///
    /// `parent_folder_id` 为 None 表示建在账号根下，否则建在它下面。
    pub async fn create_folder(
        &self,
        account_id: i64,
        parent_folder_id: Option<i64>,
        name: &str,
    ) -> Result<(), EngineError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::BadRequest("文件夹名字不能为空".to_string()));
        }

        let (delimiter, parent_server_path, parent_full_path) = {
            let store = lock_store(&self.store);
            match parent_folder_id {
                Some(id) => {
                    let folder = store
                        .get_folder_by_id(id)?
                        .ok_or_else(|| EngineError::BadRequest("父文件夹不存在或已经被删了".to_string()))?;
                    if folder.account_id != account_id {
                        return Err(EngineError::BadRequest("父文件夹不属于这个账号".to_string()));
                    }
                    if !folder_allows_children(&folder.attributes) {
                        return Err(EngineError::BadRequest(
                            "这个文件夹不允许再建子文件夹".to_string(),
                        ));
                    }
                    let server_path = folder.server_path.clone().ok_or_else(|| {
                        EngineError::BadRequest("父文件夹还没同步到服务器名，先同步一次再试".to_string())
                    })?;
                    (
                        folder.delimiter.clone(),
                        Some(server_path),
                        Some(folder.full_path.clone()),
                    )
                }
                None => {
                    let delimiter = store
                        .list_folders(account_id)?
                        .into_iter()
                        .find(|folder| !folder.delimiter.is_empty())
                        .map(|folder| folder.delimiter)
                        .unwrap_or_else(|| "/".to_string());
                    (delimiter, None, None)
                }
            }
        };

        if !delimiter.is_empty() && name.contains(&delimiter) {
            return Err(EngineError::BadRequest(format!(
                "名字里不能带分隔符「{delimiter}」；要分层请用「新建子文件夹」"
            )));
        }

        let target_server = build_server_path(parent_server_path.as_deref(), &delimiter, name);
        let target_full = match &parent_full_path {
            Some(parent) => format!("{parent}{delimiter}{name}"),
            None => name.to_string(),
        };
        {
            let store = lock_store(&self.store);
            if store.get_folder(account_id, &target_full)?.is_some() {
                return Err(EngineError::BadRequest(format!(
                    "已经有叫「{target_full}」的文件夹了"
                )));
            }
        }

        let mut client = self.connect_client(account_id).await?;
        if let Err(error) = client.create_folder(&target_server).await {
            let _ = client.logout().await;
            // 服务商明确说这里不能再分层时，把结论记到父文件夹上；
            // 下次右键不再开放入口，免得用户反复撞同一个拒绝。
            if error.kind == ConnectionErrorKind::Rejected && rejection_blocks_children(&error.message) {
                if let Some(parent_id) = parent_folder_id {
                    let store = lock_store(&self.store);
                    if let Err(mark_error) = store.set_folder_child_create_blocked(parent_id, true) {
                        tracing::warn!(error = %mark_error, "记录服务商不允许建子文件夹失败");
                    }
                }
            }
            return Err(EngineError::from(error));
        }
        let folders = client.list_folders().await?;
        let _ = client.logout().await;
        align_server_folders(&self.store, account_id, &folders)
    }

    /// 重命名一个文件夹，只改最后一段名字。
    pub async fn rename_folder(&self, folder_id: i64, new_name: &str) -> Result<(), EngineError> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err(EngineError::BadRequest("文件夹名字不能为空".to_string()));
        }

        let (account_id, delimiter, old_server, old_full) = {
            let store = lock_store(&self.store);
            let folder = store
                .get_folder_by_id(folder_id)?
                .ok_or_else(|| EngineError::BadRequest("文件夹不存在或已经被删了".to_string()))?;
            if !folder_changeable(&folder.full_path, folder.kind, &folder.attributes) {
                return Err(EngineError::BadRequest("这个文件夹不允许重命名".to_string()));
            }
            let old_server = folder.server_path.clone().ok_or_else(|| {
                EngineError::BadRequest("文件夹还没同步到服务器名，先同步一次再试".to_string())
            })?;
            (
                folder.account_id,
                folder.delimiter.clone(),
                old_server,
                folder.full_path.clone(),
            )
        };

        if !delimiter.is_empty() && new_name.contains(&delimiter) {
            return Err(EngineError::BadRequest(format!(
                "名字里不能带分隔符「{delimiter}」；要分层请用「新建子文件夹」"
            )));
        }

        let (parent_server, _) = split_parent(&old_server, &delimiter);
        let (parent_full, _) = split_parent(&old_full, &delimiter);
        let target_server = build_server_path(parent_server.as_deref(), &delimiter, new_name);
        let target_full = match &parent_full {
            Some(parent) => format!("{parent}{delimiter}{new_name}"),
            None => new_name.to_string(),
        };

        {
            let store = lock_store(&self.store);
            if let Some(existing) = store.get_folder(account_id, &target_full)? {
                if existing.id != folder_id {
                    return Err(EngineError::BadRequest(format!(
                        "已经有叫「{target_full}」的文件夹了"
                    )));
                }
            }
        }

        let mut client = self.connect_client(account_id).await?;
        if let Err(error) = client.rename_folder(&old_server, &target_server).await {
            let _ = client.logout().await;
            return Err(EngineError::from(error));
        }
        let folders = client.list_folders().await?;
        let _ = client.logout().await;
        {
            let store = lock_store(&self.store);
            store.rename_folder_prefix(
                account_id,
                &old_server,
                &target_server,
                &old_full,
                &target_full,
                &delimiter,
            )?;
        }
        align_server_folders(&self.store, account_id, &folders)
    }

    /// 删除一个空文件夹。里面有邮件或子文件夹时拒绝，不自动清空。
    pub async fn delete_folder(&self, folder_id: i64) -> Result<(), EngineError> {
        let (account_id, delimiter, server_path) = {
            let store = lock_store(&self.store);
            let folder = store
                .get_folder_by_id(folder_id)?
                .ok_or_else(|| EngineError::BadRequest("文件夹不存在或已经被删了".to_string()))?;
            if !folder_changeable(&folder.full_path, folder.kind, &folder.attributes) {
                return Err(EngineError::BadRequest("这个文件夹不允许删除".to_string()));
            }
            let server_path = folder.server_path.clone().ok_or_else(|| {
                EngineError::BadRequest("文件夹还没同步到服务器名，先同步一次再试".to_string())
            })?;
            (folder.account_id, folder.delimiter.clone(), server_path)
        };

        let mut client = self.connect_client(account_id).await?;
        let folders = client.list_folders().await?;
        if !delimiter.is_empty() {
            let child_prefix = format!("{server_path}{delimiter}");
            if folders
                .iter()
                .any(|item| item.server_path.starts_with(&child_prefix))
            {
                let _ = client.logout().await;
                return Err(EngineError::BadRequest(
                    "这个文件夹里还有子文件夹，先把子文件夹删掉".to_string(),
                ));
            }
        }

        let status = client.select(&server_path).await?;
        if status.exists > 0 {
            let _ = client.logout().await;
            return Err(EngineError::BadRequest(format!(
                "这个文件夹里还有 {} 封邮件，先清空或移走再删",
                status.exists
            )));
        }

        client.delete_folder(&server_path).await?;
        let fresh = client.list_folders().await?;
        let _ = client.logout().await;
        {
            let store = lock_store(&self.store);
            store.delete_folder(folder_id)?;
        }
        align_server_folders(&self.store, account_id, &fresh)
    }

    /// 预览删除一封邮件：判断会「移到已删除」还是「彻底删除」。
    pub fn preview_delete_message(&self, message_id: i64) -> Result<DeleteMessagePreview, EngineError> {
        let store = lock_store(&self.store);
        let location = store
            .message_location(message_id)?
            .ok_or(EngineError::MessageNotFound(message_id))?;
        let folder = store
            .get_folder_by_id(location.folder_id)?
            .ok_or_else(|| EngineError::BadRequest("邮件所在文件夹已不存在".to_string()))?;
        let trash = store
            .list_folders(location.account_id)?
            .into_iter()
            .find(|item| item.kind == FolderKind::Trash);
        if folder.kind == FolderKind::Trash || trash.is_none() {
            return Ok(DeleteMessagePreview {
                target_folder: None,
                permanent: true,
            });
        }
        Ok(DeleteMessagePreview {
            target_folder: trash.map(|item| item.full_path),
            permanent: false,
        })
    }

    /// 删除一封邮件并同步到服务商。
    ///
    /// `permanent` 为 false 时移到「已删除」；为 true 时彻底删除。
    pub async fn delete_message(&self, message_id: i64, permanent: bool) -> Result<(), EngineError> {
        let (account_id, folder_path, uid, trash_server_path) = {
            let store = lock_store(&self.store);
            let location = store
                .message_location(message_id)?
                .ok_or(EngineError::MessageNotFound(message_id))?;
            let trash = store
                .list_folders(location.account_id)?
                .into_iter()
                .find(|item| item.kind == FolderKind::Trash)
                .and_then(|item| item.server_path);
            (location.account_id, location.folder_path, location.uid, trash)
        };

        let mut client = self.connect_client(account_id).await?;
        client.select(&folder_path).await?;
        if permanent {
            client.uid_delete(uid).await?;
        } else {
            let target = trash_server_path.ok_or_else(|| {
                EngineError::BadRequest("这个账号没有「已删除」文件夹，只能彻底删除".to_string())
            })?;
            client.uid_move(uid, &target).await?;
        }
        let _ = client.logout().await;
        {
            let store = lock_store(&self.store);
            store.delete_message(message_id)?;
        }
        Ok(())
    }
}
