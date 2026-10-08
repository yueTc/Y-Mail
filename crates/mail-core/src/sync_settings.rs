//! GitHub 登录状态的落盘编排。
//!
//! 规矩（规格 3.7 / 3.8）：
//! - 访问令牌只进系统保险箱，键固定 `sync/github-token`；数据库里只允许出现这个引用键。
//! - 登录名 / 昵称 / 头像地址是公开信息，写 `setting` 表，界面直接读，不必每次请求网络。
//! - 先写保险箱再写库；写库失败要把保险箱回滚成写之前的样子，避免「有令牌没资料」的半登录态。
//! - 退出登录两处一起清。

use mail_domain::proxy::Secret;
use mail_sync::github::GitHubProfile;

use crate::engine::{EngineError, MailEngine};

/// GitHub 访问令牌在保险箱里的键（规格 3.8 固定，不许改）。
pub const GITHUB_TOKEN_KEY: &str = "sync/github-token";

/// 登录名在 `setting` 表里的键。
pub const GITHUB_LOGIN_KEY: &str = "sync.github-login";
/// 昵称在 `setting` 表里的键。
pub const GITHUB_NAME_KEY: &str = "sync.github-name";
/// 头像地址在 `setting` 表里的键。
pub const GITHUB_AVATAR_KEY: &str = "sync.github-avatar";

/// 界面要的一份登录信息（不含令牌）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubLoginView {
    /// 登录名（账号名）。
    pub login: String,
    /// 昵称；用户没设就是空。
    pub name: Option<String>,
    /// 头像地址；用户没设就是空。
    pub avatar_url: Option<String>,
}

impl GitHubLoginView {
    /// 展示名：有昵称用昵称，没有就退回登录名。
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(&self.login)
    }
}

impl MailEngine {
    /// 登录成功后落盘：令牌进保险箱，资料进库。
    ///
    /// 先写保险箱、再写库；写库中途失败会把保险箱恢复成写之前的样子。
    pub fn save_github_login(
        &self,
        token: &Secret,
        profile: &GitHubProfile,
    ) -> Result<GitHubLoginView, EngineError> {
        if token.is_empty() {
            return Err(EngineError::BadRequest("登录令牌为空".to_string()));
        }
        if !profile.is_authenticated() {
            return Err(EngineError::BadRequest("没拿到 GitHub 登录名".to_string()));
        }

        let view = GitHubLoginView {
            login: profile.login.trim().to_string(),
            name: text_field(profile.name.as_deref()),
            avatar_url: text_field(profile.avatar_url.as_deref()),
        };

        let previous = self.secrets().get(GITHUB_TOKEN_KEY)?;
        self.secrets().set(GITHUB_TOKEN_KEY, token)?;
        match self.write_login_settings(&view) {
            Ok(()) => Ok(view),
            Err(err) => {
                // 回滚：有旧令牌就还原，没有就删掉，别留半登录态。
                match previous {
                    Some(old) => {
                        let _ = self.secrets().set(GITHUB_TOKEN_KEY, &old);
                    }
                    None => {
                        let _ = self.secrets().delete(GITHUB_TOKEN_KEY);
                    }
                }
                Err(err)
            }
        }
    }

    /// 读本机已存的登录信息；没登录返回 `None`（只读库，不碰网络）。
    pub fn github_login(&self) -> Result<Option<GitHubLoginView>, EngineError> {
        let (login, name, avatar) = {
            let guard = self.store();
            (
                guard.get_setting(GITHUB_LOGIN_KEY)?,
                guard.get_setting(GITHUB_NAME_KEY)?,
                guard.get_setting(GITHUB_AVATAR_KEY)?,
            )
        };
        let Some(login) = non_empty(login) else {
            return Ok(None);
        };
        Ok(Some(GitHubLoginView {
            login,
            name: non_empty(name),
            avatar_url: non_empty(avatar),
        }))
    }

    /// 取保险箱里的 GitHub 令牌；没登录返回 `None`。
    pub fn github_token(&self) -> Result<Option<Secret>, EngineError> {
        Ok(self.secrets().get(GITHUB_TOKEN_KEY)?)
    }

    /// 退出登录：清保险箱令牌，清库里的资料行（规格 R2）。
    pub fn sign_out_github(&self) -> Result<(), EngineError> {
        self.secrets().delete(GITHUB_TOKEN_KEY)?;
        let guard = self.store();
        guard.set_setting(GITHUB_LOGIN_KEY, "")?;
        guard.set_setting(GITHUB_NAME_KEY, "")?;
        guard.set_setting(GITHUB_AVATAR_KEY, "")?;
        Ok(())
    }

    /// 把资料写进 `setting` 表；任一条失败由调用方回滚保险箱。
    fn write_login_settings(&self, view: &GitHubLoginView) -> Result<(), EngineError> {
        let guard = self.store();
        guard.set_setting(GITHUB_LOGIN_KEY, &view.login)?;
        guard.set_setting(GITHUB_NAME_KEY, view.name.as_deref().unwrap_or(""))?;
        guard.set_setting(GITHUB_AVATAR_KEY, view.avatar_url.as_deref().unwrap_or(""))?;
        Ok(())
    }
}

/// 去掉首尾空白；清空后算没设。
fn text_field(value: Option<&str>) -> Option<String> {
    non_empty(value.map(str::to_string))
}

/// 空串（或只有空白）当作没设。
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mail_domain::proxy::Secret;
    use mail_sync::github::GitHubProfile;

    use crate::engine::MailEngine;
    use crate::secrets::MemorySecretStore;

    use super::GITHUB_TOKEN_KEY;

    fn engine(dir: &std::path::Path) -> (MailEngine, Arc<MemorySecretStore>) {
        let secrets = Arc::new(MemorySecretStore::new());
        let engine = MailEngine::initialize_with_secrets(dir, secrets.clone()).expect("初始化引擎");
        (engine, secrets)
    }

    fn profile() -> GitHubProfile {
        GitHubProfile {
            login: "octocat".to_string(),
            name: Some("八爪猫".to_string()),
            avatar_url: Some("https://avatars.githubusercontent.com/u/1".to_string()),
        }
    }

    #[test]
    fn 登录后能取到资料与令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        let view = engine
            .save_github_login(&Secret::new("gho-token-123"), &profile())
            .expect("保存登录");
        assert_eq!(view.login, "octocat");
        assert_eq!(view.display_name(), "八爪猫");

        let loaded = engine.github_login().expect("读资料").expect("应已登录");
        assert_eq!(loaded, view);
        assert_eq!(
            loaded.avatar_url.as_deref(),
            Some("https://avatars.githubusercontent.com/u/1")
        );
        assert_eq!(
            engine.github_token().expect("读令牌").expect("应有令牌").expose(),
            "gho-token-123"
        );
        assert!(secrets.contains(GITHUB_TOKEN_KEY), "令牌该在保险箱里");
    }

    #[test]
    fn 退出登录后令牌和资料都清掉() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());
        engine
            .save_github_login(&Secret::new("gho-token-456"), &profile())
            .expect("保存登录");

        engine.sign_out_github().expect("退出登录");

        assert!(engine.github_login().expect("读资料").is_none(), "资料该清掉");
        assert!(engine.github_token().expect("读令牌").is_none(), "令牌该清掉");
        assert!(!secrets.contains(GITHUB_TOKEN_KEY), "保险箱里不该还有令牌");
    }

    #[test]
    fn 缺昵称时显示名退回登录名() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, _secrets) = engine(dir.path());
        let mut sparse = profile();
        sparse.name = None;
        sparse.avatar_url = None;

        let view = engine
            .save_github_login(&Secret::new("gho-token-789"), &sparse)
            .expect("保存登录");

        assert_eq!(view.display_name(), "octocat");
        assert!(view.name.is_none(), "没昵称就该是空");
        assert!(view.avatar_url.is_none(), "没头像就该是空");
        let loaded = engine.github_login().expect("读资料").expect("应已登录");
        assert_eq!(loaded, view);
    }

    #[test]
    fn 空令牌或空登录名会被挡下() {
        let dir = tempfile::tempdir().expect("临时目录");
        let (engine, secrets) = engine(dir.path());

        assert!(engine.save_github_login(&Secret::new(""), &profile()).is_err());
        let mut blank = profile();
        blank.login = "   ".to_string();
        assert!(engine
            .save_github_login(&Secret::new("gho-token-000"), &blank)
            .is_err());
        assert!(secrets.is_empty(), "被挡下的登录不该往保险箱写东西");
        assert!(engine.github_login().expect("读资料").is_none());
    }

    #[test]
    fn 数据库文件里查不到明文令牌() {
        let dir = tempfile::tempdir().expect("临时目录");
        let exposed = "gho-very-secret-token-9527";
        {
            let (engine, _secrets) = engine(dir.path());
            engine
                .save_github_login(&Secret::new(exposed), &profile())
                .expect("保存登录");
        }

        let mut found = false;
        for entry in std::fs::read_dir(dir.path()).expect("列目录") {
            let path = entry.expect("目录项").path();
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).expect("读文件");
            if bytes
                .windows(exposed.len())
                .any(|window| window == exposed.as_bytes())
            {
                found = true;
            }
        }
        assert!(!found, "任何数据文件里都不应出现明文令牌");
    }
}
