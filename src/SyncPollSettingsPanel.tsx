//! 「拉信间隔」面板：放在「账号与同步」页最上面，排在同步状态之前。
//!
//! 只管服务器不支持推送（IDLE）时的轮询间隔，单位秒，允许 10–3600，默认 60。
//! 支持推送的邮箱照旧实时收信，本项对它们没有影响。
//! 填合法值就立刻保存并生效，不用重启；保存失败回滚到改动前的值并给出提示。

import { useEffect, useState } from "react";

import { api, describeError } from "./api";
import { t } from "./i18n";

/** 允许范围与默认值，和后端校验保持一致。 */
const SYNC_POLL_MIN = 10;
const SYNC_POLL_MAX = 3600;
const SYNC_POLL_DEFAULT = 60;

/** 把输入框里的文本校验成合法秒数；不合法返回 null。 */
function parseSeconds(raw: string): number | null {
  const trimmed = raw.trim();
  if (!/^\d+$/.test(trimmed)) return null;
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < SYNC_POLL_MIN || parsed > SYNC_POLL_MAX) return null;
  return parsed;
}

export default function SyncPollSettingsPanel() {
  // 已保存的秒数与输入框文本分开存，方便编辑到一半。
  const [seconds, setSeconds] = useState(SYNC_POLL_DEFAULT);
  const [secondsText, setSecondsText] = useState(String(SYNC_POLL_DEFAULT));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");

  useEffect(() => {
    let alive = true;
    api
      .getAppSettings()
      .then((settings) => {
        if (!alive) return;
        setSeconds(settings.syncPollIntervalSeconds);
        setSecondsText(String(settings.syncPollIntervalSeconds));
      })
      .catch(() => {
        // 读不到就保持默认值，不影响其它设置。
      });
    return () => {
      alive = false;
    };
  }, []);

  /** 保存一个合法的秒数；失败回滚到改动前的值并提示。 */
  async function saveSeconds(next: number): Promise<void> {
    if (busy) return;
    const previous = seconds;
    setSeconds(next);
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const saved = await api.setSyncPollSettings(next);
      setSeconds(saved.syncPollIntervalSeconds);
      setSecondsText(String(saved.syncPollIntervalSeconds));
      setNotice(t("拉信间隔已保存，改完立刻生效。"));
    } catch (caught: unknown) {
      setSeconds(previous);
      setSecondsText(String(previous));
      setError(t("保存拉信间隔失败：{0}", [describeError(caught)]));
    } finally {
      setBusy(false);
    }
  }

  /** 输入框改动：合法就立即保存，不合法只提示、不写盘。 */
  function handleChange(raw: string) {
    setSecondsText(raw);
    const parsed = parseSeconds(raw);
    if (parsed === null) {
      setNotice("");
      setError(t("拉信间隔要填 10 到 3600 之间的整数秒。"));
      return;
    }
    setError("");
    void saveSeconds(parsed);
  }

  return (
    <section className="panel" aria-label={t("拉信间隔")}>
      <div className="panel-head">
        <h3 className="settings-subtitle">{t("拉信间隔")}</h3>
      </div>
      <div className="switch-rows">
        <label className="switch-row">
          <span className="switch-row-text">
            <span className="switch-row-title">{t("拉信间隔（秒）")}</span>
          </span>
          <input
            type="number"
            className="update-interval-input"
            min={SYNC_POLL_MIN}
            max={SYNC_POLL_MAX}
            step={1}
            inputMode="numeric"
            aria-label={t("拉信间隔（秒）")}
            value={secondsText}
            disabled={busy}
            onChange={(event) => handleChange(event.target.value)}
          />
        </label>
      </div>
      <p className="hint">
        {t("本项只在邮箱服务器不支持推送时生效；支持推送的邮箱会实时收信。")}
      </p>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {notice && (
        <p className="notice" role="status">
          {notice}
        </p>
      )}
    </section>
  );
}