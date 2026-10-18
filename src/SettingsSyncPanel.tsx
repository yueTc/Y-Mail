//! 设置同步面板（规格 3.10 / 第 4 章）。
//!
//! 始终显示同一份上传/下载表单，成功后不切换到额外控制区域。
//!
//! 安全约定：
//! - 上传与下载共用的同步密码只放在内存里，绝不写本地存储，也不进日志；
//! - 开启前必须让用户明确勾选两条风险说明；
//! - 覆盖云端或本机已有配置前必须确认。

import { useCallback, useEffect, useRef, useState } from "react";

import GitHubLoginPanel from "./GitHubLoginPanel";
import {
  api,
  describeError,
  type GitHubLoginView,
  type SettingsSyncStatus,
} from "./api";
import { t } from "./i18n";

/** 同步密码最少位数，和后端保持一致。 */
const MIN_PASSWORD_CHARS = 8;

type SettingsSyncPanelProps = {
  onProfileChanged?: (profile: GitHubLoginView | null) => void;
};

export default function SettingsSyncPanel({ onProfileChanged }: SettingsSyncPanelProps) {
  const [profile, setProfile] = useState<GitHubLoginView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [message, setMessage] = useState<string>();

  // 以下密码类输入只活在内存里，随组件卸载消失。
  const [password, setPassword] = useState("");
  const [deviceLabel, setDeviceLabel] = useState("");
  const deviceLabelEdited = useRef(false);
  const [agreePrivate, setAgreePrivate] = useState(false);
  const [agreeSecrets, setAgreeSecrets] = useState(false);

  const refreshProfile = useCallback(async () => {
    try {
      const current = await api.githubLoginProfile();
      setProfile(current);
      onProfileChanged?.(current);
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, [onProfileChanged]);

  useEffect(() => {
    void refreshProfile();
  }, [refreshProfile]);

  // 优先沿用已保存的设备名，否则填电脑名称；不覆盖用户已编辑的输入。
  useEffect(() => {
    let cancelled = false;
    const applyName = (name: string) => {
      if (cancelled) return;
      const value = name.trim() || t("本机");
      if (!deviceLabelEdited.current) setDeviceLabel(value);
    };
    const loadName = async () => {
      try {
        const [current, computerName] = await Promise.all([
          api.settingsSyncStatus(),
          api.getComputerName().catch(() => t("本机")),
        ]);
        applyName(current.deviceLabel || computerName);
      } catch (caught) {
        if (cancelled) return;
        setError(describeError(caught));
        applyName(t("本机"));
      }
    };
    void loadName();
    return () => {
      cancelled = true;
    };
  }, []);

  /** 统一处理忙状态和反馈；成功返回状态，失败或取消返回空。 */
  const run = useCallback(async (job: () => Promise<SettingsSyncStatus | undefined>) => {
    setBusy(true);
    setError(undefined);
    setMessage(undefined);
    try {
      return await job();
    } catch (caught) {
      setError(describeError(caught));
      return undefined;
    } finally {
      setBusy(false);
    }
  }, []);

  const upload = useCallback(async () => {
    if (!agreePrivate || !agreeSecrets) {
      setError(t("请先勾选上面两条说明。"));
      return;
    }
    if ([...password].length < MIN_PASSWORD_CHARS) {
      setError(t("同步密码至少 8 位。"));
      return;
    }
    if (!deviceLabel.trim()) {
      setError(t("请填写这台设备的名字。"));
      return;
    }
    const next = await run(async () => {
      try {
        return await api.settingsSyncEnable(password, deviceLabel.trim(), false);
      } catch (caught) {
        if (!describeError(caught).includes("云端已有同步内容")) throw caught;
        const confirmed = window.confirm(
          t("云端已有同步内容，是否用本机配置覆盖？覆盖后，云端内容将使用当前同步密码加密。"),
        );
        if (!confirmed) return undefined;
        return api.settingsSyncEnable(password, deviceLabel.trim(), true);
      }
    });
    if (next) {
      setMessage(t("已上传到云端。"));
    }
  }, [agreePrivate, agreeSecrets, password, deviceLabel, run]);

  const download = useCallback(async () => {
    if (!password) {
      setError(t("请填写同步密码。"));
      return;
    }
    if (!deviceLabel.trim()) {
      setError(t("请填写这台设备的名字。"));
      return;
    }
    const next = await run(async () => {
      try {
        return await api.settingsSyncDownload(password, deviceLabel.trim(), false);
      } catch (caught) {
        const message = describeError(caught);
        if (!message.includes("云端配置会覆盖本机这些条目")) throw caught;
        if (!window.confirm(t("下载将覆盖本机的同名配置，是否继续？\n{0}", [message]))) return undefined;
        return api.settingsSyncDownload(password, deviceLabel.trim(), true);
      }
    });
    if (next) {
      setMessage(t("已从云端下载。"));
    }
  }, [password, deviceLabel, run]);

  const hasSyncScope = profile?.scope === "sync";

  return (
    <section className="panel settings-sync" aria-busy={loading || busy}>
      <div className="panel-head">
        <h2>{t("同步")}</h2>
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

          {hasSyncScope && (
            <>
              <div className="settings-sync-enable">
                <p className="hint">
                  {t("上传后，账号、代理、签名、界面开关、AI 配置和密钥会先加密，再保存到你自己的私密 Gist。")}
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
                    onChange={(event) => {
                      deviceLabelEdited.current = true;
                      setDeviceLabel(event.target.value);
                    }}
                    placeholder={t("例如：家里的台式机")}
                  />
                </label>
                <label>
                  {t("同步密码")}
                  <input
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                </label>
                <p className="hint">{t("同步密码至少 8 位，请妥善保存。")}</p>
                <div className="actions">
                  <button className="primary" disabled={busy} onClick={() => void upload()}>
                    {t("上传到云端")}
                  </button>
                  <button disabled={busy} onClick={() => void download()}>
                    {t("从云端下载")}
                  </button>
                </div>
              </div>
            </>
          )}
        </>
      )}

      {message && <p className="hint" role="status">{message}</p>}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}