import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { api, describeError, type AppSettings } from "./api";
import AboutPanel from "./AboutPanel";
import AccountPanel from "./AccountPanel";
import AiPanel from "./AiPanel";
import AppearanceSettingsPanel from "./AppearanceSettingsPanel";
import McpPanel from "./McpPanel";
import PaneResizer from "./PaneResizer";
import ProxyPanel from "./ProxyPanel";
import ReaderSettingsPanel from "./ReaderSettingsPanel";
import RestartChoiceDialog from "./RestartChoiceDialog";
import SyncPanel from "./SyncPanel";
import { RAIL_WIDTH, RESIZER_WIDTH } from "./usePaneWidths";
import { t } from "./i18n";

type StorageState =
  | { kind: "loading" }
  | { kind: "ready"; settings: AppSettings }
  | { kind: "error"; message: string };

/** 左侧分类编号；顺序见 SETTINGS_CATEGORIES。 */
type SettingsCategoryId =
  | "general"
  | "appearance"
  | "accounts"
  | "proxy"
  | "ai"
  | "mcp"
  | "storage"
  | "contacts"
  | "about";

/** 左侧分类，数组顺序即界面顺序；文案进 `t()` 取词条。 */
const SETTINGS_CATEGORIES: { id: SettingsCategoryId; label: string }[] = [
  { id: "general", label: "通用" },
  { id: "appearance", label: "外观" },
  { id: "accounts", label: "账号与同步" },
  { id: "proxy", label: "代理" },
  { id: "ai", label: "AI功能" },
  { id: "mcp", label: "MCP" },
  { id: "storage", label: "存储与通知" },
  { id: "contacts", label: "通讯录" },
  { id: "about", label: "关于" },
];

/** 分类栏宽度记忆：默认 200，范围 160–320。 */
const SETTINGS_NAV_STORAGE_KEY = "ymail.settings-nav-width.v1";
const SETTINGS_NAV_DEFAULT = 200;
const SETTINGS_NAV_MIN = 160;
const SETTINGS_NAV_MAX = 320;
/** 右边内容区至少留这么宽；窗口太窄时先挤分类栏。 */
const SETTINGS_CONTENT_MIN = 420;
/** 设置页左右留白，算可用宽度时要扣掉。 */
const SETTINGS_PADDING_X = 48;

function clampNavWidth(value: number): number {
  return Math.min(SETTINGS_NAV_MAX, Math.max(SETTINGS_NAV_MIN, Math.round(value)));
}

function readStoredNavWidth(): number {
  try {
    const raw = window.localStorage.getItem(SETTINGS_NAV_STORAGE_KEY);
    if (!raw) return SETTINGS_NAV_DEFAULT;
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "number" || !Number.isFinite(parsed)) return SETTINGS_NAV_DEFAULT;
    return clampNavWidth(parsed);
  } catch {
    return SETTINGS_NAV_DEFAULT;
  }
}

function persistNavWidth(value: number): void {
  try {
    window.localStorage.setItem(SETTINGS_NAV_STORAGE_KEY, JSON.stringify(clampNavWidth(value)));
  } catch {
    // 存不下就算了，不阻塞使用。
  }
}

type SettingsWorkspaceProps = {
  proxiesVersion: number;
  onProxiesChanged: () => void;
};

/**
 * 设置模式：左边分类、右边内容的双栏设置页。
 * 这里只做分类和排版，具体功能复用已有面板，不复制第二套逻辑。
 * 切换分类只隐藏面板、不卸载，避免丢掉没保存的表单内容。
 */
export default function SettingsWorkspace({
  proxiesVersion,
  onProxiesChanged,
}: SettingsWorkspaceProps) {
  const [storage, setStorage] = useState<StorageState>({ kind: "loading" });
  const [dataDir, setDataDir] = useState("");
  const [notifyNewMail, setNotifyNewMail] = useState(true);
  // 读信是否默认拦住远程图片；默认拦，保存后以后端返回值为准。
  const [blockRemoteImagesByDefault, setBlockRemoteImagesByDefault] = useState(true);
  const [saving, setSaving] = useState(false);
  const [migrating, setMigrating] = useState(false);
  const [opening, setOpening] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // 通讯录分组的清空结果提示与忙碌标记。
  const [contactNotice, setContactNotice] = useState("");
  const [contactBusy, setContactBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // 「通用」里的远程图片开关单独一份提示，别和存储分组互相串。
  const [generalNotice, setGeneralNotice] = useState<string | null>(null);
  const [generalSaveError, setGeneralSaveError] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  // 开机启动：状态以后端真实状态为准，不落本地设置。
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  // 迁移成功后的不可关闭重启选择弹窗。
  const [restartNeeded, setRestartNeeded] = useState(false);
  const [restartBusy, setRestartBusy] = useState(false);
  const [restartError, setRestartError] = useState<string | null>(null);
  // 左栏选中项：只在本次运行内记住，默认选「通用」。
  const [activeCategory, setActiveCategory] = useState<SettingsCategoryId>("general");
  // 左栏宽度：拖动时只改内存，松手或键盘调整后落盘。
  const [navWidth, setNavWidth] = useState<number>(() => readStoredNavWidth());
  const navWidthRef = useRef(navWidth);
  const [windowWidth, setWindowWidth] = useState<number>(() =>
    typeof window === "undefined" ? 1440 : window.innerWidth,
  );

  useEffect(() => {
    let cancelled = false;

    api
      .getAppSettings()
      .then((settings) => {
        if (cancelled) return;
        setStorage({ kind: "ready", settings });
        setDataDir(settings.dataDir);
        setNotifyNewMail(settings.notifyNewMail);
        setBlockRemoteImagesByDefault(settings.blockRemoteImagesByDefault);
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

  // 窗口变窄时优先保住右边内容宽度，左栏自动收窄到最小。
  useEffect(() => {
    const onResize = () => setWindowWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const navMax = Math.max(
    SETTINGS_NAV_MIN,
    Math.min(
      SETTINGS_NAV_MAX,
      windowWidth - RAIL_WIDTH - SETTINGS_PADDING_X - RESIZER_WIDTH - SETTINGS_CONTENT_MIN,
    ),
  );
  const effectiveNavWidth = Math.min(navMax, clampNavWidth(navWidth));

  /** 拖动中只改内存，够快；松手时再落盘。 */
  function changeNavWidth(next: number) {
    const clamped = clampNavWidth(next);
    navWidthRef.current = clamped;
    setNavWidth(clamped);
  }

  function commitNavWidth() {
    persistNavWidth(navWidthRef.current);
  }

  function resetNavWidth() {
    navWidthRef.current = SETTINGS_NAV_DEFAULT;
    setNavWidth(SETTINGS_NAV_DEFAULT);
    persistNavWidth(SETTINGS_NAV_DEFAULT);
  }

  /**
   * 保存通知开关与「默认拦截远程图片」；数据目录只能通过“更改目录”迁移。
   * `scope` 区分是哪个分组点的保存：两个分组同时挂着，提示只在自己那块显示。
   */
  async function saveStorage(
    event: React.FormEvent<HTMLFormElement>,
    scope: "general" | "storage",
  ) {
    event.preventDefault();
    if (storage.kind !== "ready") return;
    const setGroupNotice = scope === "general" ? setGeneralNotice : setNotice;
    const setGroupError = scope === "general" ? setGeneralSaveError : setSaveError;
    setSaving(true);
    setGroupNotice(null);
    setGroupError(null);
    try {
      const saved = await api.saveAppSettings({
        dataDir: storage.settings.dataDir,
        attachmentDir: "",
        notifyNewMail,
        blockRemoteImagesByDefault,
      });
      setStorage({ kind: "ready", settings: saved });
      setNotifyNewMail(saved.notifyNewMail);
      setBlockRemoteImagesByDefault(saved.blockRemoteImagesByDefault);
      setGroupNotice(t("设置已保存。"));
    } catch (error: unknown) {
      setGroupError(describeError(error));
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
        title: t("选择新的数据目录"),
      });
      picked = typeof chosen === "string" ? chosen : null;
    } catch (error: unknown) {
      setDirError(t("打开目录选择失败：{0}", [describeError(error)]));
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
        throw new Error(result.message || t("目标目录需要确认"));
      }
      const saved = await api.getAppSettings();
      setStorage({ kind: "ready", settings: saved });
      setDataDir(saved.dataDir);
      setRestartNeeded(true);
    } catch (error: unknown) {
      setDirError(t("目录切换失败：{0}", [describeError(error)]));
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
      setRestartError(t("重启失败，请手动关掉再打开：{0}", [describeError(error)]));
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
      setNotice(t("已打开当前数据目录。"));
    } catch (error: unknown) {
      setDirError(t("打开目录失败：{0}", [describeError(error)]));
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
        t("清掉所有同步自动收集的联系人？你自己建或改过的、以及已隐藏的都不会动。"),
      )
    ) {
      return;
    }
    setContactBusy(true);
    try {
      const removed = await api.clearAutoContacts();
      setContactNotice(t("已清掉 {0} 条自动收集的联系人", [removed]));
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
          <h1>{t("设置")}</h1>
          <p className="subtitle">
            {t("授权码只进 Windows 凭据管理器，保存前会先做一次连接自检。")}</p>
        </div>
      </header>

      <div
        className="settings-columns"
        style={{
          gridTemplateColumns: `${effectiveNavWidth}px ${RESIZER_WIDTH}px minmax(0, 1fr)`,
        }}
      >
        <nav className="settings-nav" aria-label={t("设置分类")}>
          <ul className="settings-nav-list">
            {SETTINGS_CATEGORIES.map((category) => {
              const active = category.id === activeCategory;
              return (
                <li key={category.id}>
                  <button
                    type="button"
                    className={active ? "settings-nav-item active" : "settings-nav-item"}
                    aria-current={active ? "true" : undefined}
                    onClick={() => setActiveCategory(category.id)}
                  >
                    {t(category.label)}
                  </button>
                </li>
              );
            })}
          </ul>
        </nav>

        <PaneResizer
          label={t("设置分类栏宽度")}
          value={effectiveNavWidth}
          min={SETTINGS_NAV_MIN}
          max={navMax}
          onChange={changeNavWidth}
          onCommit={commitNavWidth}
          onReset={resetNavWidth}
        />

        <div className="settings-content">
          <section
            className="settings-group"
            aria-label={t("通用")}
            hidden={activeCategory !== "general"}
          >
            <h2 className="settings-group-title">{t("通用")}</h2>

            <section className="panel" aria-label={t("开机自动启动")}>
              <h3 className="settings-subtitle">{t("开机自动启动")}</h3>
              <label className="checkbox">
                <input
                  type="checkbox"
                  checked={autostart}
                  disabled={autostartBusy}
                  onChange={(event) => void toggleAutostart(event.target.checked)}
                />
                <span>{t("开机自动启动（静默进托盘，不弹主窗口）")}</span>
              </label>
              <p className="hint">
                {t("打开后会把本程序写进 Windows 当前用户的启动项，开机自动在后台收信， 只留一个托盘图标。你在任务管理器的启动项里手动禁用，这里也会跟着显示成关闭。")}</p>
              {autostartError && (
                <p className="error" role="alert">
                  {t("开机启动设置失败：")}{autostartError}
                </p>
              )}
            </section>

            <ReaderSettingsPanel />

            <section className="panel" aria-label={t("默认拦截远程图片")}>
              {storage.kind === "ready" ? (
                <form
                  className="settings-form"
                  onSubmit={(event) => void saveStorage(event, "general")}
                >
                  <label className="checkbox">
                    <input
                      type="checkbox"
                      checked={blockRemoteImagesByDefault}
                      onChange={(event) => setBlockRemoteImagesByDefault(event.target.checked)}
                    />
                    <span>{t("默认拦截邮件里的远程图片")}</span>
                  </label>
                  <p className="hint">
                    {t("远程图片会暴露你什么时候打开邮件。默认拦；关掉后读信会直接加载图片，风险自负。")}</p>
                  <div className="actions">
                    <button type="submit" disabled={saving} aria-busy={saving}>
                      {t("保存设置")}</button>
                  </div>
                  {generalNotice && (
                    <p className="hint" role="status">
                      {generalNotice}
                    </p>
                  )}
                  {generalSaveError && (
                    <p className="error" role="alert">
                      {t("保存失败：")}{generalSaveError}
                    </p>
                  )}
                </form>
              ) : (
                <p className="hint" role="status">
                  {t("正在读取……")}</p>
              )}
            </section>
          </section>

          <section
            className="settings-group"
            aria-label={t("外观")}
            hidden={activeCategory !== "appearance"}
          >
            <h2 className="settings-group-title">{t("外观")}</h2>
            <AppearanceSettingsPanel />
          </section>

          <section
            className="settings-group"
            aria-label={t("账号与同步")}
            hidden={activeCategory !== "accounts"}
          >
            <h2 className="settings-group-title">{t("账号与同步")}</h2>
            <SyncPanel />
            <AccountPanel proxiesVersion={proxiesVersion} />
          </section>

          <section
            className="settings-group"
            aria-label={t("代理")}
            hidden={activeCategory !== "proxy"}
          >
            <h2 className="settings-group-title">{t("代理")}</h2>
            <ProxyPanel onChanged={onProxiesChanged} />
          </section>

          <section
            className="settings-group"
            aria-label={t("AI功能")}
            hidden={activeCategory !== "ai"}
          >
            <h2 className="settings-group-title">{t("AI功能")}</h2>
            <AiPanel />
          </section>

          <section
            className="settings-group"
            aria-label={t("MCP")}
            hidden={activeCategory !== "mcp"}
          >
            <h2 className="settings-group-title">{t("MCP")}</h2>
            <McpPanel />
          </section>

          <section
            className="settings-group"
            aria-label={t("存储与通知")}
            hidden={activeCategory !== "storage"}
          >
            <h2 className="settings-group-title">{t("存储与通知")}</h2>
            <section
              className="panel"
              aria-busy={storage.kind === "loading" || migrating || opening}
            >
              {storage.kind === "loading" && (
                <p className="hint" role="status">
                  {t("正在读取……")}</p>
              )}
              {storage.kind === "error" && (
                <p className="error" role="alert">
                  {t("读取失败：")}{storage.message}
                </p>
              )}
              {storage.kind === "ready" && (
                <form
                  className="settings-form"
                  onSubmit={(event) => void saveStorage(event, "storage")}
                >
                  <label className="field">
                    <span>{t("数据目录")}</span>
                    <input
                      type="text"
                      value={dataDir}
                      placeholder={storage.settings.defaultDataDir}
                      onChange={(event) => setDataDir(event.target.value)}
                      spellCheck={false}
                    />
                    <small className="hint">
                      {t("数据库和日志放这里。改目录会先复制并校验，重启后生效。留空用默认：")}{storage.settings.defaultDataDir}
                    </small>
                  </label>

                  <p className="hint">
                    {t("下载文件保存在：")}<span className="path">{storage.settings.defaultAttachmentDir}</span>
                  </p>

                  <label className="checkbox">
                    <input
                      type="checkbox"
                      checked={notifyNewMail}
                      onChange={(event) => setNotifyNewMail(event.target.checked)}
                    />
                    <span>{t("新邮件用 Windows 系统通知提醒（窗口切到后台时才弹）")}</span>
                  </label>

                  <dl className="status">
                    <dt>{t("当前生效的数据目录")}</dt>
                    <dd className="path">{storage.settings.activeDataDir}</dd>
                    <dt>{t("当前生效的下载目录")}</dt>
                    <dd className="path">{storage.settings.activeAttachmentDir}</dd>
                  </dl>

                  <div className="actions">
                    <button
                      type="submit"
                      disabled={saving || migrating || opening}
                      aria-busy={saving}
                    >
                      {t("保存设置")}</button>
                    <button
                      type="button"
                      onClick={changeDirectory}
                      disabled={saving || migrating || opening}
                      aria-busy={migrating}
                    >
                      {t("更改目录")}</button>
                    <button
                      type="button"
                      onClick={openDirectory}
                      disabled={saving || migrating || opening}
                      aria-busy={opening}
                    >
                      {t("打开目录")}</button>
                  </div>

                  {migrating && (
                    <p className="hint" role="status">
                      {t("正在更改目录，请稍等……")}</p>
                  )}
                  {opening && (
                    <p className="hint" role="status">
                      {t("正在打开目录……")}</p>
                  )}
                  {notice && (
                    <p className="hint" role="status">
                      {notice}
                    </p>
                  )}
                  {saveError && (
                    <p className="error" role="alert">
                      {t("保存失败：")}{saveError}
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

          <section
            className="settings-group"
            aria-label={t("通讯录")}
            hidden={activeCategory !== "contacts"}
          >
            <h2 className="settings-group-title">{t("通讯录")}</h2>
            <section className="panel">
              <p className="hint">
                {t("通讯录里的联系人分两种：收信发信时自动记下来的，和你自己建或改过的。 这条只清自动记下来的那些；你自己建或改过的、以及已经藏起来的一条都不动。")}</p>
              <div className="form-actions">
                <button type="button" className="danger" disabled={contactBusy} onClick={() => void clearAutoContacts()}>
                  {t("清空自动收集的联系人")}</button>
              </div>
              {contactNotice ? <p className="notice">{contactNotice}</p> : null}
            </section>
          </section>

          <section
            className="settings-group"
            aria-label={t("关于")}
            hidden={activeCategory !== "about"}
          >
            <h2 className="settings-group-title">{t("关于")}</h2>
            <AboutPanel />
          </section>
        </div>
      </div>

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