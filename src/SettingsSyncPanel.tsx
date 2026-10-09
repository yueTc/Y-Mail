//! 设置同步面板（规格 3.10 / 第 4 章）。
//!
//! 这里管三件事：GitHub 登录入口、开启/关闭同步、立即同步与冲突处理。
//!
//! 安全约定：
//! - 同步密码与新设备加入用的密码只放在内存里，绝不写本地存储，也不进日志；
//! - 开启前必须让用户明确勾选两条风险说明；
//! - Gist 链接只交给系统浏览器打开，不在界面内嵌远程内容。

import { useCallback, useEffect, useState } from "react";

import GitHubLoginPanel from "./GitHubLoginPanel";
import {
  api,
  describeError,
  type ConflictChoice,
  type GitHubLoginView,
  type SettingsSyncStatus,
} from "./api";
import { t } from "./i18n";

/** 同步密码最少位数，和后端保持一致。 */
const MIN_PASSWORD_CHARS = 8;

export default function SettingsSyncPanel() {
  const [profile, setProfile] = useState<GitHubLoginView | null>(null);
  const [status, setStatus] = useState<SettingsSyncStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  // 以下密码类输入只活在内存里，随组件卸载消失。
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [deviceLabel, setDeviceLabel] = useState("");
  const [agreePrivate, setAgreePrivate] = useState(false);
  const [agreeSecrets, setAgreeSecrets] = useState(false);

  const [joinPassword, setJoinPassword] = useState("");
  const [joinLabel, setJoinLabel] = useState("");

  const [newPassword, setNewPassword] = useState("");
  const [newConfirm, setNewConfirm] = useState("");

  const refreshProfile = useCallback(async () => {
    try {
      setProfile(await api.githubLoginProfile());
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      setStatus(await api.settingsSyncStatus());
    } catch (caught) {
      setError(describeError(caught));
    }
  }, []);

  useEffect(() => {
    void refreshProfile();
    void refreshStatus();
  }, [refreshProfile, refreshStatus]);

  /** 统一包一层：忙状态、错误提示、状态刷新；成功返回新状态，失败返回空。 */
  const run = useCallback(async (job: () => Promise<SettingsSyncStatus>) => {
    setBusy(true);
    setError(undefined);
    try {
      const next = await job();
      setStatus(next);
      return next;
    } catch (caught) {
      setError(describeError(caught));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);

  const enable = useCallback(async () => {
    if (!agreePrivate || !agreeSecrets) {
      setError(t("请先勾选上面两条说明。"));
      return;
    }
    if (password !== confirm) {
      setError(t("两次输入的同步密码不一致。"));
      return;
    }
    if (password.length < MIN_PASSWORD_CHARS) {
      setError(t("同步密码至少 8 位。"));
      return;
    }
    if (!deviceLabel.trim()) {
      setError(t("请填写这台设备的名字。"));
      return;
    }
    const next = await run(() => api.settingsSyncEnable(password, confirm, deviceLabel.trim()));
    if (next) {
      setPassword("");
      setConfirm("");
    }
  }, [agreePrivate, agreeSecrets, password, confirm, deviceLabel, run]);

  const join = useCallback(async () => {
    if (!joinPassword) {
      setError(t("请填写同步密码。"));
      return;
    }
    if (!joinLabel.trim()) {
      setError(t("请填写这台设备的名字。"));
      return;
    }
    const next = await run(() =>
      api.settingsSyncJoin(joinPassword, joinLabel.trim(), true),
    );
    if (next) {
      setJoinPassword("");
    }
  }, [joinPassword, joinLabel, run]);

  const resetPassword = useCallback(async () => {
    if (newPassword !== newConfirm) {
      setError(t("两次输入的同步密码不一致。"));
      return;
    }
    if (newPassword.length < MIN_PASSWORD_CHARS) {
      setError(t("同步密码至少 8 位。"));
      return;
    }
    const next = await run(() => api.settingsSyncResetPassword(newPassword, newConfirm));
    if (next) {
      setNewPassword("");
      setNewConfirm("");
    }
  }, [newPassword, newConfirm, run]);

  const resolve = useCallback(
    (choice: ConflictChoice) => {
      void run(() => api.settingsSyncResolveConflict(choice));
    },
    [run],
  );

  const openGist = useCallback(async (url: string) => {
    try {
      await api.openExternalUrl(url);
    } catch (caught) {
      setError(describeError(caught));
    }
  }, []);

  /** 关闭同步：默认保留云端，勾选后才连云端一起删。 */
  const [deleteRemote, setDeleteRemote] = useState(false);

  const hasSyncScope = profile?.scope === "sync";
  const enabled = status?.enabled === true;

  return (
    <section className="panel settings-sync" aria-busy={loading || busy}>
      <div className="panel-head">
        <h2>{t("设置同步")}</h2>
        {enabled && (
          <div className="actions">
            <button className="primary" disabled={busy} onClick={() => void run(() => api.settingsSyncNow())}>
              {t("立即同步")}
            </button>
          </div>
        )}
      </div>

      {loading && <p className="hint" role="status">{t("正在读取设置同步状态……")}</p>}

      {!loading && !profile && (
        <>
          <p className="hint">{t("设置同步需要先登录 GitHub。只想显示头像也可以只登录，不开启同步。")}</p>
          <GitHubLoginPanel scope="login" onChanged={() => void refreshProfile()} />
        </>
      )}

      {!loading && profile && (
        <>
          <GitHubLoginPanel
            scope={hasSyncScope ? "login" : "sync"}
            onChanged={() => void refreshProfile()}
          />

          {!hasSyncScope && (
            <p className="hint" role="status">
              {t("设置同步需要「管理你的 Gist」权限，请在上面再授权一次。")}
            </p>
          )}

          {hasSyncScope && status?.conflict && (
            <div className="settings-sync-conflict" role="alert">
              <p>
                {t("检测到冲突：另一台设备「{0}」也改过配置（版本 {1}）。请选择保留哪一边。", [
                  status.conflict.remoteDeviceLabel,
                  status.conflict.remoteRevision,
                ])}
              </p>
              <div className="actions">
                <button disabled={busy} onClick={() => resolve("keepLocal")}>
                  {t("保留本机配置")}
                </button>
                <button disabled={busy} onClick={() => resolve("keepRemote")}>
                  {t("采用远端配置")}
                </button>
              </div>
            </div>
          )}

          {hasSyncScope && enabled && (
            <>
              <p className="settings-sync-line">
                <span>{t("设备名：")}{status?.deviceLabel || t("本机")}</span>
                <span>{t("当前版本：")}{status?.revision ?? 0}</span>
                <span>
                  {t("上次同步：")}
                  {status?.lastSyncAt || t("还没同步过")}
                </span>
              </p>
              {status?.gistUrl && (
                <div className="actions">
                  <button type="button" onClick={() => void openGist(status.gistUrl!)}>
                    {t("在浏览器里打开 Gist")}
                  </button>
                </div>
              )}

              <div className="settings-sync-reset">
                <details>
                  <summary>{t("重设同步密码")}</summary>
                  <p className="hint">
                    {t("重设后会新建一个 Gist 并删掉旧的；旧密码将再也解不开新数据。")}
                  </p>
                  <label>
                    {t("新同步密码")}
                    <input
                      type="password"
                      autoComplete="new-password"
                      value={newPassword}
                      onChange={(event) => setNewPassword(event.target.value)}
                    />
                  </label>
                  <label>
                    {t("再输一次")}
                    <input
                      type="password"
                      autoComplete="new-password"
                      value={newConfirm}
                      onChange={(event) => setNewConfirm(event.target.value)}
                    />
                  </label>
                  <div className="actions">
                    <button disabled={busy} onClick={() => void resetPassword()}>
                      {t("重设密码")}
                    </button>
                  </div>
                </details>
              </div>

              <label className="settings-sync-check">
                <input
                  type="checkbox"
                  checked={deleteRemote}
                  onChange={(event) => setDeleteRemote(event.target.checked)}
                />
                {t("关闭时连云端那份一起删（不勾选就只关本机，云端留着）")}
              </label>
              <div className="actions">
                <button disabled={busy} onClick={() => void run(() => api.settingsSyncDisable(deleteRemote))}>
                  {t("关闭同步")}
                </button>
              </div>
            </>
          )}

          {hasSyncScope && !enabled && (
            <>
              <div className="settings-sync-enable">
                <p className="hint">
                  {t("开启后，账号、代理、签名、界面开关、AI 配置和密钥会先加密，再上传到你自己的私密 Gist。")}
                </p>
                <label className="settings-sync-check">
                  <input
                    type="checkbox"
                    checked={agreePrivate}
                    onChange={(event) => setAgreePrivate(event.target.checked)}
                  />
                  {t("我明白「私密 Gist」并非真正私有，拿到链接的人能看到内容（内容是加密的，但传输记录会留下）。")}
                </label>
                <label className="settings-sync-check">
                  <input
                    type="checkbox"
                    checked={agreeSecrets}
                    onChange={(event) => setAgreeSecrets(event.target.checked)}
                  />
                  {t("我明白同步内容里包含 AI 密钥等敏感信息，会一并加密上传。")}
                </label>
                <label>
                  {t("这台设备的名字")}
                  <input
                    type="text"
                    value={deviceLabel}
                    onChange={(event) => setDeviceLabel(event.target.value)}
                    placeholder={t("例如：家里的台式机")}
                  />
                </label>
                <label>
                  {t("同步密码")}
                  <input
                    type="password"
                    autoComplete="new-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                </label>
                <label>
                  {t("再输一次")}
                  <input
                    type="password"
                    autoComplete="new-password"
                    value={confirm}
                    onChange={(event) => setConfirm(event.target.value)}
                  />
                </label>
                <div className="actions">
                  <button className="primary" disabled={busy} onClick={() => void enable()}>
                    {t("开启同步")}
                  </button>
                </div>
              </div>

              <details className="settings-sync-join">
                <summary>{t("这是第二台设备？输入同步密码加入")}</summary>
                <label>
                  {t("同步密码")}
                  <input
                    type="password"
                    autoComplete="current-password"
                    value={joinPassword}
                    onChange={(event) => setJoinPassword(event.target.value)}
                  />
                </label>
                <label>
                  {t("这台设备的名字")}
                  <input
                    type="text"
                    value={joinLabel}
                    onChange={(event) => setJoinLabel(event.target.value)}
                    placeholder={t("例如：公司的笔记本")}
                  />
                </label>
                <div className="actions">
                  <button disabled={busy} onClick={() => void join()}>
                    {t("用同步密码加入")}
                  </button>
                </div>
              </details>
            </>
          )}
        </>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}