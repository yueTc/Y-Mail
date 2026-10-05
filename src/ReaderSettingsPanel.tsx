//! 读信安全设置：查看 / 移除「记住的发件人」——这些发件人的远程图片会自动显示。
//!
//! 只读取本地设置里的名单，不碰邮件正文，也不发任何网络请求。

import { useCallback, useEffect, useState } from "react";

import { api, describeError } from "./api";

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
    <section className="panel">
      <div className="panel-head">
        <h2>读信与远程图片</h2>
        <p className="hint">
          远程图片默认拦截。被记住的发件人会直接显示图片，要恢复拦截就在下面移除。
        </p>
      </div>

      {error && <p className="error">操作失败：{error}</p>}
      {loading && <p className="hint">正在读取……</p>}
      {!loading && senders.length === 0 && (
        <p className="hint">
          还没有记住的发件人。在读信页点「以后这个发件人都自动显示」即可添加。
        </p>
      )}

      {senders.length > 0 && (
        <ul className="trusted-senders">
          {senders.map((address) => (
            <li key={address}>
              <span className="path">{address}</span>
              <button
                type="button"
                disabled={busy === address}
                onClick={() => void remove(address)}
              >
                {busy === address ? "移除中……" : "移除"}
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}