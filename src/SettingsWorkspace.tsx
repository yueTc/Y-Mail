import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { api, describeError, type AppSettings, type DbStatus } from "./api";
import AccountPanel from "./AccountPanel";
import AiPanel from "./AiPanel";
import AppearanceSettingsPanel from "./AppearanceSettingsPanel";
import McpPanel from "./McpPanel";
import ProxyPanel from "./ProxyPanel";
import ReaderSettingsPanel from "./ReaderSettingsPanel";
import RestartChoiceDialog from "./RestartChoiceDialog";
import SyncPanel from "./SyncPanel";

type LoadState =
  | { kind: "loading" }
  | { kind: "ready"; status: DbStatus }
  | { kind: "error"; message: string };

type StorageState =
  | { kind: "loading" }
  | { kind: "ready"; settings: AppSettings }
  | { kind: "error"; message: string };

type SettingsWorkspaceProps = {
  proxiesVersion: number;
  onProxiesChanged: () => void;
  /** 从设置页回到收件箱；由外壳传入。 */
  onGoInbox: () => void;
};

/**
 * 设置模式：单栏设置页，整块占满右侧区域。
 * 这里只做分组和排版，具体功能复用已有面板，不复制第二套逻辑。
 */
export default function SettingsWorkspace({
  proxiesVersion,
  onProxiesChanged,
  onGoInbox,
}: SettingsWorkspaceProps) {
  const [state, setState] = useState<LoadState>({ kind: "loading" });
  const [storage, setStorage] = useState<StorageState>({ kind: "loading" });
  const [dataDir, setDataDir] = useState("");
  const [notifyNewMail, setNotifyNewMail] = useState(true);
  const [saving, setSaving] = useState(false);
  const [migrating, setMigrating] = useState(false);
  const [opening, setOpening] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // 通讯录分组的清空结果提示与忙碌标记。
  const [contactNotice, setContactNotice] = useState("");
  const [contactBusy, setContactBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  // 开机启动：状态以后端真实状态为准，不落本地设置。
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  // 迁移成功后的不可关闭重启选择弹窗。
  const [restartNeeded, setRestartNeeded] = useState(false);
  const [restartBusy, setRestartBusy] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    api
      .dbStatus()
      .then((status) => {
        if (!cancelled) setState({ kind: "ready", status });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ kind: "error", message: describeError(error) });
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    api
      .getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        setStorage({ kind: "ready", settings });
        setDataDir(settings.dataDir);
        setNotifyNewMail(settings.notifyNewMail);
      })
      .catch((error: unknown) => {
        if (!cancelled) setStorage({ kind: "error", message: describeError(error) });
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;

    api
      .autostartStatus()
      .then((enabled) => {
        if (!cancelled) setAutostart(enabled);
      })
      .catch((error: unknown) => {
        if (!cancelled) setAutostartError(describeError(error));
      });

    return () => {
      cancelled = true;
    };
  }, []);

  /** 只保存通知开关；数据目录只能通过“更改目录”迁移，不能悄悄换。 */
  async function saveStorage(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (storage.kind !== "ready") return;
    setSaving(true);
    setNotice(null);
    setSaveError(null);
    try {
      const saved = await api.saveAppSettings({
        dataDir: storage.settings.dataDir,
        attachmentDir: "",
        notifyNewMail,
      });
      setStorage({ kind: "ready", settings: saved });
      setNotifyNewMail(saved.notifyNewMail);
      setNotice("通知设置已保存。");
    } catch (error: unknown) {
      setSaveError(describeError(error));
    } finally {
      setSaving(false);
    }
  }

  /** 先弹系统目录选择框，选完再复制、校验；全部通过以后后端才写设置。 */
  async function changeDirectory() {
    setNotice(null);
    setDirError(null);
    let picked: string | null = null;
    try {
      const chosen = await open({
        directory: true,
        multiple: false,
        title: "选择新的数据目录",
      });
      picked = typeof chosen === "string" ? chosen : null;
    } catch (error: unknown) {
      setDirError(`打开目录选择失败：${describeError(error)}`);
      return;
    }
    if (!picked) return; // 用户在系统对话框里点了取消
    setDataDir(picked);
    setMigrating(true);
    try {
      let result = await api.changeDataDir(picked, false);
      if (result.needsConfirmation) {
        if (!window.confirm(result.message)) return;
        result = await api.changeDataDir(picked, true);
      }
      if (result.needsConfirmation) {
        throw new Error(result.message || "目标目录需要确认");
      }
      const saved = await api.getAppSettings();
      setStorage({ kind: "ready", settings: saved });
      setDataDir(saved.dataDir);
      setRestartNeeded(true);
    } catch (error: unknown) {
      setDirError(`目录切换失败：${describeError(error)}`);
    } finally {
      setMigrating(false);
    }
  }

  /** 迁移完成后由用户明确选择是否清理旧目录，再重启。 */
  async function restart(cleanup: boolean) {
    setRestartError(null);
    setRestartBusy(true);
    try {
      await api.restartApp(cleanup);
    } catch (error: unknown) {
      setRestartError(`重启失败，请手动关掉再打开：${describeError(error)}`);
    } finally {
      setRestartBusy(false);
    }
  }

  /** 只打开后端当前生效的目录，前端不能指定任意路径。 */
  async function openDirectory() {
    setOpening(true);
    setNotice(null);
    setDirError(null);
    try {
      await api.openDataDir();
      setNotice("已打开当前数据目录。");
    } catch (error: unknown) {
      setDirError(`打开目录失败：${describeError(error)}`);
    } finally {
      setOpening(false);
    }
  }

  /** 勾选 / 取消开机启动：写完回读真实状态；失败就回读真实状态并报错。 */
  async function toggleAutostart(next: boolean) {
    if (autostartBusy) return;
    const previous = autostart;
    setAutostartBusy(true);
    setAutostartError(null);
    try {
      setAutostart(await api.setAutostart(next));
    } catch (error: unknown) {
      setAutostartError(describeError(error));
      try {
        setAutostart(await api.autostartStatus());
      } catch {
        setAutostart(previous);
      }
    } finally {
      setAutostartBusy(false);
    }
  }

  /** 清掉自动收集的联系人；手动的和已隐藏的后端自己会跳过。 */
  const clearAutoContacts = async () => {
    if (contactBusy) return;
    if (
      !window.confirm(
        "清掉所有同步自动收集的联系人？你自己建或改过的、以及已隐藏的都不会动。",
      )
    ) {
      return;
    }
    setContactBusy(true);
    try {
      const removed = await api.clearAutoContacts();
      setContactNotice(`已清掉 ${removed} 条自动收集的联系人`);
    } catch (error: unknown) {
      setContactNotice(describeError(error));
    } finally {
      setContactBusy(false);
    }
  };

  return (
    <div className="settings-workspace">
      <header className="settings-head">
        <div>
          <h1>设置</h1>
          <p className="subtitle">
            授权码只进 Windows 凭据管理器，保存前会先做一次连接自检。
          </p>
        </div>
        <button type="button" onClick={onGoInbox}>
          进入收件箱
        </button>
      </header>

      <section className="settings-group" aria-label="外观">
        <h2 className="settings-group-title">外观</h2>
        <AppearanceSettingsPanel />
      </section>

      <section className="settings-group" aria-label="启动">
        <h2 className="settings-group-title">启动</h2>
        <section className="panel">
          <label className="checkbox">
            <input
              type="checkbox"
              checked={autostart}
              disabled={autostartBusy}
              onChange={(event) => void toggleAutostart(event.target.checked)}
            />
            <span>开机自动启动（静默进托盘，不弹主窗口）</span>
          </label>
          <p className="hint">
            打开后会把本程序写进 Windows 当前用户的启动项，开机自动在后台收信，
            只留一个托盘图标。你在任务管理器的启动项里手动禁用，这里也会跟着显示成关闭。
          </p>
          {autostartError && (
            <p className="error" role="alert">
              开机启动设置失败：{autostartError}
            </p>
          )}
        </section>
      </section>

      <section className="settings-group" aria-label="账号与同步">
        <h2 className="settings-group-title">账号与同步</h2>
        <SyncPanel />
        <AccountPanel proxiesVersion={proxiesVersion} />
      </section>

      <section className="settings-group" aria-label="代理">
        <h2 className="settings-group-title">代理</h2>
        <ProxyPanel onChanged={onProxiesChanged} />
      </section>

      <section className="settings-group" aria-label="阅读设置">
        <h2 className="settings-group-title">阅读设置</h2>
        <ReaderSettingsPanel />
      </section>

      <section className="settings-group" aria-label="人工智能">
        <h2 className="settings-group-title">人工智能</h2>
        <AiPanel />
      </section>

      <section className="settings-group" aria-label="外部接入">
        <h2 className="settings-group-title">外部接入</h2>
        <McpPanel />
      </section>

      <section className="settings-group" aria-label="存储目录与通知">
        <h2 className="settings-group-title">存储目录与通知</h2>
        <section
          className="panel"
          aria-busy={storage.kind === "loading" || migrating || opening}
        >
          {storage.kind === "loading" && (
            <p className="hint" role="status">
              正在读取……
            </p>
          )}
          {storage.kind === "error" && (
            <p className="error" role="alert">
              读取失败：{storage.message}
            </p>
          )}
          {storage.kind === "ready" && (
            <form className="settings-form" onSubmit={saveStorage}>
              <label className="field">
                <span>数据目录</span>
                <input
                  type="text"
                  value={dataDir}
                  placeholder={storage.settings.defaultDataDir}
                  onChange={(event) => setDataDir(event.target.value)}
                  spellCheck={false}
                />
                <small className="hint">
                  数据库和日志放这里。改目录会先复制并校验，重启后生效。留空用默认：
                  {storage.settings.defaultDataDir}
                </small>
              </label>

              <p className="hint">
                下载文件保存在：<span className="path">{storage.settings.defaultAttachmentDir}</span>
              </p>

              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={notifyNewMail}
                  onChange={(event) => setNotifyNewMail(event.target.checked)}
                />
                <span>新邮件用 Windows 系统通知提醒（窗口切到后台时才弹）</span>
              </label>

              <dl className="status">
                <dt>当前生效的数据目录</dt>
                <dd className="path">{storage.settings.activeDataDir}</dd>
                <dt>当前生效的下载目录</dt>
                <dd className="path">{storage.settings.activeAttachmentDir}</dd>
              </dl>

              <div className="actions">
                <button
                  type="submit"
                  disabled={saving || migrating || opening}
                  aria-busy={saving}
                >
                  保存通知设置
                </button>
                <button
                  type="button"
                  onClick={changeDirectory}
                  disabled={saving || migrating || opening}
                  aria-busy={migrating}
                >
                  更改目录
                </button>
                <button
                  type="button"
                  onClick={openDirectory}
                  disabled={saving || migrating || opening}
                  aria-busy={opening}
                >
                  打开目录
                </button>
              </div>

              {migrating && (
                <p className="hint" role="status">
                  正在更改目录，请稍等……
                </p>
              )}
              {opening && (
                <p className="hint" role="status">
                  正在打开目录……
                </p>
              )}
              {notice && (
                <p className="hint" role="status">
                  {notice}
                </p>
              )}
              {saveError && (
                <p className="error" role="alert">
                  保存失败：{saveError}
                </p>
              )}
              {dirError && (
                <p className="error" role="alert">
                  {dirError}
                </p>
              )}
            </form>
          )}
        </section>
      </section>

      <section className="settings-group" aria-label="通讯录">
        <h2 className="settings-group-title">通讯录</h2>
        <section className="panel">
          <p className="hint">
            通讯录里的联系人分两种：收信发信时自动记下来的，和你自己建或改过的。
            这条只清自动记下来的那些；你自己建或改过的、以及已经藏起来的一条都不动。
          </p>
          <div className="form-actions">
            <button type="button" className="danger" disabled={contactBusy} onClick={() => void clearAutoContacts()}>
              清空自动收集的联系人
            </button>
          </div>
          {contactNotice ? <p className="notice">{contactNotice}</p> : null}
        </section>
      </section>

      <section className="settings-group" aria-label="数据库状态">
        <h2 className="settings-group-title">数据库状态</h2>
        <section className="panel" aria-busy={state.kind === "loading"}>
          {state.kind === "loading" && <p className="hint" role="status">正在读取……</p>}
          {state.kind === "error" && <p className="error" role="alert">读取失败：{state.message}</p>}
          {state.kind === "ready" && (
            <dl className="status">
              <dt>数据库文件</dt>
              <dd className="path">{state.status.databaseFile}</dd>
              <dt>结构版本</dt>
              <dd>{state.status.schemaVersion}</dd>
              <dt>已登记迁移</dt>
              <dd>
                {state.status.appliedVersions.join("、") || "无"}
                （共 {state.status.appliedVersions.length} 条）
              </dd>
              <dt>本次新应用</dt>
              <dd>{state.status.appliedCount} 条</dd>
              <dt>全文检索 FTS5</dt>
              <dd>{state.status.fts5Available ? "可用" : "不可用"}</dd>
              <dt>附件目录</dt>
              <dd className="path">{state.status.attachmentDir}</dd>
              <dt>日志目录</dt>
              <dd className="path">{state.status.logDir}</dd>
            </dl>
          )}
        </section>
      </section>

      {restartNeeded && (
        <RestartChoiceDialog
          busy={restartBusy}
          error={restartError}
          onChoose={(cleanup) => void restart(cleanup)}
        />
      )}
    </div>
  );
}