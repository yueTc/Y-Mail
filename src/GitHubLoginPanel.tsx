//! GitHub 登录面板：设备码登录、复制用户码、轮询结果、显示头像与昵称、退出登录。
//!
//! 安全约定：
//! - 令牌只存在后端与系统保险箱里，前端状态里只有公开资料；
//! - 头像只从固定域名 `avatars.githubusercontent.com` 加载，其余地址一律不渲染。

import { useCallback, useEffect, useRef, useState } from "react";

import {
  api,
  describeError,
  type GitHubDeviceLoginView,
  type GitHubLoginView,
  type GitHubScope,
} from "./api";
import { writeClipboard } from "./externalAttachments";
import { t } from "./i18n";

/** 允许加载头像的固定域名（规格 3.7：全程序唯一放行的远程图片）。 */
const AVATAR_HOST = "avatars.githubusercontent.com";

/**
 * 只放行 GitHub 头像域名；协议必须是 https，其它一律返回 undefined、不加载。
 *
 * 导出是为了让单测能直接验证拦截逻辑。
 */
export function safeAvatarUrl(url: string | null | undefined): string | undefined {
  if (!url) return undefined;
  try {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:") return undefined;
    if (parsed.hostname !== AVATAR_HOST) return undefined;
    return parsed.toString();
  } catch {
    return undefined;
  }
}

type Props = {
  /**
   * 要申请的权限档：`login` 只要头像昵称，`sync` 再加 Gist（开启设置同步用）。
   * 如果本机已登录的权限弱于这一档，面板会给出「再授权一次」的入口。
   */
  scope?: GitHubScope;
  /** 登录成功或退出后通知外层刷新。 */
  onChanged?: () => void;
};

export default function GitHubLoginPanel({ scope = "login", onChanged }: Props) {
  const [profile, setProfile] = useState<GitHubLoginView | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [device, setDevice] = useState<GitHubDeviceLoginView | null>(null);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string>();

  // 轮询用 timeout（不是 interval）：每轮结果可能要求拉长间隔。
  const timerRef = useRef<number | undefined>(undefined);
  const intervalRef = useRef(5);

  const refresh = useCallback(async () => {
    try {
      const current = await api.githubLoginProfile();
      setProfile(current);
      setError(undefined);
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const stopPolling = useCallback(() => {
    if (timerRef.current !== undefined) {
      window.clearTimeout(timerRef.current);
      timerRef.current = undefined;
    }
  }, []);

  useEffect(() => stopPolling, [stopPolling]);

  const poll = useCallback(
    async (loginId: string) => {
      try {
        const result = await api.githubLoginPoll(loginId);
        if (result.status === "authorized") {
          stopPolling();
          setDevice(null);
          setProfile({
            login: result.login,
            name: result.name,
            avatarUrl: result.avatarUrl,
            scope: result.scope,
          });
          setError(undefined);
          onChanged?.();
          return;
        }
        if (result.status === "expired" || result.status === "denied") {
          stopPolling();
          setDevice(null);
          setError(
            result.status === "expired"
              ? t("这组登录码已过期，请重新发起登录")
              : t("你在浏览器里取消了授权"),
          );
          return;
        }
        if (result.status === "slowDown") intervalRef.current += 5;
        timerRef.current = window.setTimeout(() => void poll(loginId), intervalRef.current * 1000);
      } catch (caught) {
        stopPolling();
        setDevice(null);
        setError(describeError(caught));
      }
    },
    [onChanged, stopPolling],
  );

  const start = useCallback(async () => {
    setBusy(true);
    setError(undefined);
    setCopied(false);
    try {
      const view = await api.githubLoginStart(scope);
      intervalRef.current = view.interval > 0 ? view.interval : 5;
      setDevice(view);
      void poll(view.loginId);
      try {
        await api.openExternalUrl(view.verificationUri);
      } catch {
        // 打不开浏览器不算失败，用户还能手动点「重新打开浏览器」。
      }
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [poll, scope]);

  const reopen = useCallback(async () => {
    if (!device) return;
    try {
      await api.openExternalUrl(device.verificationUri);
    } catch (caught) {
      setError(describeError(caught));
    }
  }, [device]);

  const copy = useCallback(async () => {
    if (!device) return;
    setCopied(await writeClipboard(device.userCode));
  }, [device]);

  const signOut = useCallback(async () => {
    setBusy(true);
    setError(undefined);
    try {
      await api.githubLoginSignOut();
      stopPolling();
      setProfile(null);
      setDevice(null);
      onChanged?.();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  }, [onChanged, stopPolling]);

  const avatar = safeAvatarUrl(profile?.avatarUrl);
  const displayName = profile ? profile.name?.trim() || profile.login : "";
  /**
   * 已登录的权限够不够这一档。
   * `sync` 权限包含 `login`，所以已登录 `sync` 时再要 `login` 也算够。
   */
  const scopeSatisfied = !profile || profile.scope === "sync" || scope === "login";

  return (
    <div className="github-login" aria-busy={loading || busy}>
      {loading && (
        <p className="hint" role="status">
          {t("正在读取 GitHub 登录状态……")}
        </p>
      )}

      {!loading && profile && scopeSatisfied && (
        <div className="github-account">
          {avatar ? (
            <img
              className="github-avatar"
              src={avatar}
              alt={t("GitHub 头像")}
              width={40}
              height={40}
              referrerPolicy="no-referrer"
            />
          ) : (
            <span className="github-avatar github-avatar-fallback" aria-hidden="true">
              {displayName.slice(0, 1).toUpperCase()}
            </span>
          )}
          <div className="github-account-name">
            <strong>{displayName}</strong>
            <span className="hint">{"@" + profile.login}</span>
          </div>
          <button type="button" disabled={busy} onClick={() => void signOut()}>
            {t("退出登录")}
          </button>
        </div>
      )}

      {!loading && !profile && !device && (
        <div className="actions">
          <button type="button" className="primary" disabled={busy} onClick={() => void start()}>
            {t("用 GitHub 登录")}
          </button>
        </div>
      )}

      {!loading && profile && !scopeSatisfied && !device && (
        <div className="github-upgrade">
          <p className="hint">
            {t("设置同步需要额外的「管理你的 Gist」权限，请再授权一次。")}
          </p>
          <div className="actions">
            <button type="button" className="primary" disabled={busy} onClick={() => void start()}>
              {t("再授权一次（需要 Gist 权限）")}
            </button>
          </div>
        </div>
      )}

      {device && (
        <div className="github-device" role="group" aria-label={t("GitHub 设备码")}>
          <p className="hint">{t("在浏览器里输入下面的用户码完成授权：")}</p>
          <div className="github-code-row">
            <code className="github-code">{device.userCode}</code>
            <button type="button" onClick={() => void copy()}>
              {copied ? t("已复制") : t("复制")}
            </button>
            <button type="button" onClick={() => void reopen()}>
              {t("重新打开浏览器")}
            </button>
          </div>
          <p className="hint" role="status">
            {t("等待你在浏览器里确认……")}
          </p>
        </div>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}