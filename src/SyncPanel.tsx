//! 同步面板：显示每个账号的同步状态，提供启动、停止与进度查看。
//!
//! 只调外壳命令拿内存快照；不接触凭据，也不把邮件正文带进界面状态。

import { useCallback, useEffect, useState } from "react";

import { api, describeError, type SyncStatus } from "./api";
import { t } from "./i18n";

/** 界面轮询间隔：两秒看一次状态快照就够，不要给后台添乱。 */
const POLL_MS = 2000;

/** 把进度数字整理成一句人话。 */
export function progressText(status: SyncStatus): string {
  if (status.total > 0) return t("已同步 {0} 封 / 共 {1} 封", [status.progress, status.total]);
  if (status.progress > 0) return t("已同步 {0} 封", [status.progress]);
  return "";
}

export default function SyncPanel() {
  const [statuses, setStatuses] = useState<SyncStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const refresh = useCallback(async () => {
    try {
      const list = await api.syncStatus();
      setStatuses(list);
      setError(undefined);
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;

    const boot = async () => {
      try {
        // 应用起来就把已启用账号的同步跑起来；已经在跑的会被自动跳过。
        await api.startSync();
      } catch (caught) {
        if (!cancelled) setError(describeError(caught));
      }
      if (cancelled) return;
      await refresh();
      if (!cancelled) timer = window.setInterval(() => void refresh(), POLL_MS);
    };

    void boot();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearInterval(timer);
    };
  }, [refresh]);

  /** 「立即同步」= 先停掉旧线程，再从头起一个，保证断点与状态都刷新。 */
  const restart = async (accountId?: number) => {
    setBusy(true);
    setError(undefined);
    try {
      await api.stopSync(accountId);
      await api.startSync(accountId);
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  };

  /** 停止一个账号或全部停止。 */
  const stop = async (accountId?: number) => {
    setBusy(true);
    setError(undefined);
    try {
      await api.stopSync(accountId);
      await refresh();
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="panel" aria-busy={loading || busy}>
      <div className="panel-head">
        <h2>{t("同步状态")}</h2>
        <div className="actions">
          <button className="primary" disabled={busy} onClick={() => void restart()}>
            {t("全部立即同步")}</button>
          <button disabled={busy} onClick={() => void stop()}>
            {t("全部停止")}</button>
        </div>
      </div>

      {error && <p className="error" role="alert">{t("同步操作失败：")}{error}</p>}

      {loading && statuses.length === 0 && <p className="hint" role="status">{t("正在读取同步状态……")}</p>}
      {!loading && statuses.length === 0 && (
        <p className="hint">{t("还没有可同步的账号。先在下面添加一个邮箱账号。")}</p>
      )}

      {statuses.map((item) => (
        <article className="sync-row" key={item.accountId}>
          <div className="sync-line">
            <strong>{item.email}</strong>
            <span className={item.needsReauth ? "badge danger" : "badge"}>{item.stateLabel}</span>
          </div>
          <p className="sync-detail" role="status">
            {progressText(item) && <span>{progressText(item)}</span>}
            {item.message && <span className="hint">{item.message}</span>}
            {!progressText(item) && !item.message && <span className="hint">{t("暂无进度")}</span>}
          </p>
          {item.needsReauth && (
            <p className="error" role="alert">{t("授权码失效或缺失，请到账号设置里重新填写后再同步。")}</p>
          )}
          <div className="actions">
            <button disabled={busy} onClick={() => void restart(item.accountId)}>
              {t("立即同步")}</button>
            <button disabled={busy} onClick={() => void stop(item.accountId)}>
              {t("停止")}</button>
          </div>
        </article>
      ))}
    </section>
  );
}