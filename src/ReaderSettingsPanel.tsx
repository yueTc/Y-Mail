//! 读信设置：查看 / 移除「记住的发件人」——这些发件人的远程图片会自动显示。
//!
//! 只读写本地名单，不碰邮件正文，也不发任何网络请求。设置页一直挂载，所以只在
//! 用户重新进入「通用」时刷新，避免名单停在启动时的旧状态。

import { useCallback, useEffect, useState } from "react";

import { api, describeError } from "./api";
import { t } from "./i18n";

type ReaderSettingsPanelProps = {
  /** 当前是否正在显示「通用」分组；隐藏时不发请求，重新显示时刷新名单。 */
  active?: boolean;
};

export default function ReaderSettingsPanel({ active = true }: ReaderSettingsPanelProps) {
  const [senders, setSenders] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(undefined);
    try {
      setSenders(await api.listTrustedRemoteSenders());
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return;
    void refresh();
  }, [active, refresh]);

  const remove = async (address: string) => {
    setBusy(address);
    setError(undefined);
    try {
      setSenders(await api.forgetRemoteSender(address));
    } catch (caught) {
      setError(describeError(caught));
    } finally {
      setBusy(undefined);
    }
  };

  return (
    <section className="panel" aria-label={t("读信与远程图片")} aria-busy={loading || busy !== undefined}>
      <div className="panel-head">
        <div>
          <h2>{t("读信与远程图片")}</h2>
        </div>
      </div>

      {error && <p className="error" role="alert">{t("操作失败：")}{error}</p>}
      {loading && <p className="hint" role="status">{t("正在读取……")}</p>}

      {!loading && !error && senders.length === 0 && (
        <p className="hint" role="status">
          {t("暂无记住的发件人。读信时点「以后这个发件人都自动显示」，这里就会出现可移除的地址。")}
        </p>
      )}

      {senders.length > 0 && (
        <ul className="trusted-senders">
          {senders.map((address) => (
            <li key={address}>
              <span className="path">{address}</span>
              <button
                type="button"
                aria-busy={busy === address}
                disabled={busy === address}
                onClick={() => void remove(address)}
              >
                {busy === address ? t("移除中……") : t("移除")}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}