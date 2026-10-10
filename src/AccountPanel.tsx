import { useCallback, useEffect, useState } from "react";

import { api, describeError, type Account } from "./api";
import { PROXY_MODE_LABELS } from "./AccountForm";
import AccountDialog from "./AccountDialog";
import { t } from "./i18n";

interface Props {
  /** 代理列表变化时由外壳递增，用来触发本面板重新拉取可选代理。 */
  proxiesVersion: number;
}

export default function AccountPanel({ proxiesVersion }: Props) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 「新增 / 编辑邮箱」弹窗开关；两种操作都走弹窗。 */
  const [addOpen, setAddOpen] = useState(false);
  const [editing, setEditing] = useState<Account | null>(null);

  const reload = useCallback(async () => {
    try {
      const list = await api.listAccounts();
      setAccounts(list);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  function openCreate() {
    setError(null);
    setNotice(null);
    setEditing(null);
    setAddOpen(true);
  }

  function openEdit(account: Account) {
    setError(null);
    setNotice(null);
    setEditing(account);
  }

  /** 弹窗保存成功后：收起弹窗、刷新账号列表；新增和编辑的提示文案不一样。 */
  function handleSaved(isNew: boolean) {
    setNotice(isNew ? t("账号已保存。") : t("账号已更新。"));
    setAddOpen(false);
    setEditing(null);
    void reload();
  }

  async function handleSavedTest(account: Account) {
    setBusy(`test-${account.id}`);
    setError(null);
    setNotice(null);
    try {
      const result = await api.testSavedAccount(account.id);
      setNotice(
        t("「{0}」自检通过：收件服务器能看到 {1} 个文件夹，发件认证方式 {2}。", [account.displayName, result.imapFolderCount, result.smtpMechanism]),
      );
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDelete(account: Account) {
    if (!window.confirm(t("确定删除账号「{0}」吗？系统凭据管理器里的授权码会一并删除。", [account.displayName]))) {
      return;
    }
    setBusy(`delete-${account.id}`);
    setError(null);
    setNotice(null);
    try {
      await api.deleteAccount(account.id);
      if (editing !== null && editing.id === account.id) setEditing(null);
      setNotice(t("已删除账号「{0}」。", [account.displayName]));
      await reload();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  return (
    <section className="panel" aria-busy={accounts === null || busy !== null}>
      <div className="panel-head">
        <h2>{t("邮箱账号")}</h2>
        <button type="button" onClick={openCreate} disabled={busy !== null}>
          {t("新增账号")}</button>
      </div>

      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      {accounts === null ? (
        <p className="hint" role="status">{t("正在读取账号……")}</p>
      ) : accounts.length === 0 ? (
        <p className="hint">{t("还没有账号。点「新增账号」填一个，保存前会先做连接自检。")}</p>
      ) : (
        <ul className="card-list">
          {accounts.map((account) => (
            <li key={account.id} className="card">
              <span className="dot" style={{ background: account.color || "#3b82f6" }} />
              <div className="card-main">
                <div className="card-title">
                  {account.displayName}
                  {!account.enabled && <span className="tag">{t("已停用")}</span>}
                  <span className="tag">{t(PROXY_MODE_LABELS[account.proxy.mode])}</span>
                  {!account.hasCredential && <span className="tag warn">{t("缺授权码")}</span>}
                </div>
                <div className="card-sub">
                  {account.email}
                  {account.username !== account.email ? t("（登录名 {0}）", [account.username]) : ""}
                </div>
                <div className="card-sub path">
                  {t("收 ")}
                  {account.imap.host}:{account.imap.port}
                  {t(" · 发 ")}
                  {account.smtp.host}:{account.smtp.port}
                </div>
              </div>
              <div className="card-actions">
                <button type="button" onClick={() => openEdit(account)} disabled={busy !== null}>
                  {t("编辑")}</button>
                <button
                  type="button"
                  onClick={() => void handleSavedTest(account)}
                  aria-busy={busy === `test-${account.id}`}
                  disabled={busy !== null}
                >
                  {busy === `test-${account.id}` ? t("自检中……") : t("自检")}
                </button>
                <button
                  type="button"
                  className="danger"
                  onClick={() => void handleDelete(account)}
                  aria-busy={busy === `delete-${account.id}`}
                  disabled={busy !== null}
                >
                  {busy === `delete-${account.id}` ? t("删除中……") : t("删除")}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      <AccountDialog
        open={addOpen}
        source="settings"
        account={null}
        proxiesVersion={proxiesVersion}
        onClose={() => setAddOpen(false)}
        onSaved={() => handleSaved(true)}
      />

      <AccountDialog
        open={editing !== null}
        source="settings"
        account={editing}
        proxiesVersion={proxiesVersion}
        onClose={() => setEditing(null)}
        onSaved={() => handleSaved(false)}
      />
    </section>
  );
}