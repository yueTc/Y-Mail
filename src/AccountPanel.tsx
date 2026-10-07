import { useCallback, useEffect, useState } from "react";

import { api, describeError, type Account } from "./api";
import AccountForm, { PROXY_MODE_LABELS } from "./AccountForm";
import AddAccountDialog from "./AddAccountDialog";

interface Props {
  /** 代理列表变化时由外壳递增，用来触发本面板重新拉取可选代理。 */
  proxiesVersion: number;
}

export default function AccountPanel({ proxiesVersion }: Props) {
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  /** 「添加邮箱」弹窗开没开；新增走弹窗，编辑仍在本面板内联。 */
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

  /** 表单保存成功后：收起表单、刷新账号列表。弹窗来源与内联编辑的提示文案不一样。 */
  function handleSaved(fromDialog: boolean) {
    setNotice(fromDialog ? "账号已保存。" : "账号已更新。");
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
        `「${account.displayName}」自检通过：收件服务器能看到 ${result.imapFolderCount} 个文件夹，发件认证方式 ${result.smtpMechanism}。`,
      );
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(null);
    }
  }

  async function handleDelete(account: Account) {
    if (!window.confirm(`确定删除账号「${account.displayName}」吗？系统凭据管理器里的授权码会一并删除。`)) {
      return;
    }
    setBusy(`delete-${account.id}`);
    setError(null);
    setNotice(null);
    try {
      await api.deleteAccount(account.id);
      if (editing !== null && editing.id === account.id) setEditing(null);
      setNotice(`已删除账号「${account.displayName}」。`);
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
        <h2>邮箱账号</h2>
        <button type="button" onClick={openCreate} disabled={busy !== null}>
          新增账号
        </button>
      </div>

      {error && <p className="error" role="alert">{error}</p>}
      {notice && <p className="notice" role="status">{notice}</p>}

      {accounts === null ? (
        <p className="hint" role="status">正在读取账号……</p>
      ) : accounts.length === 0 ? (
        <p className="hint">还没有账号。点「新增账号」填一个，保存前会先做连接自检。</p>
      ) : (
        <ul className="card-list">
          {accounts.map((account) => (
            <li key={account.id} className="card">
              <span className="dot" style={{ background: account.color || "#3b82f6" }} />
              <div className="card-main">
                <div className="card-title">
                  {account.displayName}
                  {!account.enabled && <span className="tag">已停用</span>}
                  <span className="tag">{PROXY_MODE_LABELS[account.proxy.mode]}</span>
                  {!account.hasCredential && <span className="tag warn">缺授权码</span>}
                </div>
                <div className="card-sub">
                  {account.email}
                  {account.username !== account.email ? `（登录名 ${account.username}）` : ""}
                </div>
                <div className="card-sub path">
                  收 {account.imap.host}:{account.imap.port} · 发 {account.smtp.host}:{account.smtp.port}
                </div>
              </div>
              <div className="card-actions">
                <button type="button" onClick={() => openEdit(account)} disabled={busy !== null}>
                  编辑
                </button>
                <button
                  type="button"
                  onClick={() => void handleSavedTest(account)}
                  aria-busy={busy === `test-${account.id}`}
                  disabled={busy !== null}
                >
                  {busy === `test-${account.id}` ? "自检中……" : "自检"}
                </button>
                <button
                  type="button"
                  className="danger"
                  onClick={() => void handleDelete(account)}
                  aria-busy={busy === `delete-${account.id}`}
                  disabled={busy !== null}
                >
                  {busy === `delete-${account.id}` ? "删除中……" : "删除"}
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {editing !== null && (
        <>
          <h3>{`编辑账号 #${editing.id}`}</h3>
          <AccountForm
            key={editing.id}
            account={editing}
            source="settings"
            proxiesVersion={proxiesVersion}
            onSaved={() => handleSaved(false)}
            onCancel={() => setEditing(null)}
          />
        </>
      )}

      <AddAccountDialog
        open={addOpen}
        source="settings"
        proxiesVersion={proxiesVersion}
        onClose={() => setAddOpen(false)}
        onSaved={() => handleSaved(true)}
      />
    </section>
  );
}