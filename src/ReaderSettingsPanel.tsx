//! 读信设置：查看 / 移除「记住的发件人」——这些发件人的远程图片会自动显示。
//!
//! 只读写本地名单，不碰邮件正文，也不发任何网络请求。深色模式已经搬到「外观」分组。

import { useCallback, useEffect, useState } from "react";

import { api, describeError } from "./api";
import { t } from "./i18n";

export default function ReaderSettingsPanel() {
  const [senders, setSenders] = useState<string[]>([]);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string>();
  const [error, setError] = useState<string>();

  const refresh = useCallback(async () => {
    try {
      setSenders(await api.listTrustedRemoteSenders());
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
        <h2>{t("读信与远程图片")}</h2>
        <p className="hint">
          {t("远程图片默认拦截。被记住的发件人会直接显示图片，要恢复拦截就在下面移除。")}</p>
      </div>

      {error && <p className="error" role="alert">{t("操作失败：")}{error}</p>}
      {loading && <p className="hint" role="status">{t("正在读取……")}</p>}
      {!loading && senders.length === 0 && (
        <p className="hint">
          {t("还没有记住的发件人。在读信页点「以后这个发件人都自动显示」即可添加。")}</p>
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