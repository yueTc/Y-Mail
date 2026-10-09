//! 关于：看当前版本、打开项目主页、查更新并就地热更新。
//!
//! 更新走 Tauri 官方更新插件：更新包带发布方签名，下载后先验签再安装，装完重启。
//! 应用里只放公钥，改不了更新包；网址写死在本文件，界面传不进任意链接。
//!
//! 这里还有「自动检测更新」的开关和间隔。自动检查的定时器不在这里，放在最外层
//! `App.tsx`，因为用户没打开设置页时也要能检查、点亮左栏小红点。

import { useEffect, useState } from "react";

import { api, describeError, type UpdateInfo } from "./api";
import { t } from "./i18n";

/** 项目主页（源码、问题反馈）。 */
const GITHUB_HOME = "https://github.com/yueTc/Y-Mail";
/** 下载页：指向最新一次发布。 */
const GITHUB_RELEASES = "https://github.com/yueTc/Y-Mail/releases/latest";

/** 检测间隔允许范围（小时）与默认值，跟后端校验保持一致。 */
const UPDATE_INTERVAL_MIN = 1;
const UPDATE_INTERVAL_MAX = 168;
const UPDATE_INTERVAL_DEFAULT = 24;

/** 查更新这一步的四种状态；分开写是为了让界面只显示该显示的那句。 */
type CheckState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "found"; info: UpdateInfo }
  | { kind: "failed"; message: string };

type AboutPanelProps = {
  /** 手动检查查到新版本时上报版本号，用来点亮设置页左栏红点。 */
  onUpdateFound?: (version: string) => void;
  /** 自动检测开关或间隔改动后上报，用来重排最外层的定时器。 */
  onUpdateSettingsChanged?: (enabled: boolean, intervalHours: number) => void;
};

export default function AboutPanel({ onUpdateFound, onUpdateSettingsChanged }: AboutPanelProps) {
  // 版本号以后端读到的为准，不写死常量。
  const [version, setVersion] = useState("");
  const [versionError, setVersionError] = useState("");
  const [state, setState] = useState<CheckState>({ kind: "idle" });
  // 下载安装阶段的进度与错误；百分比读不到时就是 null（有些服务器不给总长度）。
  const [installing, setInstalling] = useState(false);
  const [percent, setPercent] = useState<number | null>(null);
  const [installError, setInstallError] = useState("");
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState("");
  // 自动检测：已保存的开关与间隔，以及间隔输入框里的文本（分开存方便编辑到一半）。
  const [autoCheckUpdate, setAutoCheckUpdate] = useState(false);
  const [intervalHours, setIntervalHours] = useState(UPDATE_INTERVAL_DEFAULT);
  const [intervalText, setIntervalText] = useState(String(UPDATE_INTERVAL_DEFAULT));
  const [updateSettingsBusy, setUpdateSettingsBusy] = useState(false);
  const [updateSettingsError, setUpdateSettingsError] = useState("");

  useEffect(() => {
    let alive = true;
    api
      .appVersion()
      .then((value) => {
        if (alive) setVersion(value);
      })
      .catch((error: unknown) => {
        if (alive) setVersionError(describeError(error));
      });
    return () => {
      alive = false;
    };
  }, []);

  // 读一次已保存的自动检测设置来填开关和间隔；读失败就用默认（关、24 小时）。
  useEffect(() => {
    let alive = true;
    api
      .getAppSettings()
      .then((settings) => {
        if (!alive) return;
        setAutoCheckUpdate(settings.autoCheckUpdate);
        setIntervalHours(settings.updateCheckIntervalHours);
        setIntervalText(String(settings.updateCheckIntervalHours));
      })
      .catch(() => {
        // 读不到设置不影响手动检查更新，保持默认值就行。
      });
    return () => {
      alive = false;
    };
  }, []);

  /** 用系统浏览器打开一个写死的页面；失败只提示，不影响别处。 */
  async function openUrl(url: string) {
    if (opening) return;
    setOpening(true);
    setOpenError("");
    try {
      await api.openExternalUrl(url);
    } catch (error: unknown) {
      setOpenError(t("打开链接失败：{0}", [describeError(error)]));
    } finally {
      setOpening(false);
    }
  }

  async function checkUpdate() {
    if (state.kind === "checking" || installing) return;
    setState({ kind: "checking" });
    setInstallError("");
    try {
      const info = await api.checkForUpdate();
      if (info) {
        setState({ kind: "found", info });
        // 手动查到的也按同一套规则点亮左栏红点。
        onUpdateFound?.(info.version);
      } else {
        setState({ kind: "latest" });
      }
    } catch (error: unknown) {
      setState({ kind: "failed", message: describeError(error) });
    }
  }

  async function installUpdate() {
    if (state.kind !== "found" || installing) return;
    setInstalling(true);
    setInstallError("");
    setPercent(null);
    try {
      await api.installPendingUpdate((value) => setPercent(value));
      // 装完重启才进新版本；Windows 上主程序可能已经被安装程序结束，走不到这里。
      await api.relaunchApp();
    } catch (error: unknown) {
      setInstallError(t("安装更新失败：{0}", [describeError(error)]));
      setInstalling(false);
      setPercent(null);
    }
  }

  /** 保存自动检测开关与间隔；失败就回滚并提示，成功后通知外层重排定时器。 */
  async function saveUpdateSettings(nextEnabled: boolean, nextHours: number): Promise<void> {
    if (updateSettingsBusy) return;
    const previousEnabled = autoCheckUpdate;
    const previousHours = intervalHours;
    setAutoCheckUpdate(nextEnabled);
    setIntervalHours(nextHours);
    setUpdateSettingsBusy(true);
    setUpdateSettingsError("");
    try {
      const saved = await api.setUpdateSettings(nextEnabled, nextHours);
      setAutoCheckUpdate(saved.autoCheckUpdate);
      setIntervalHours(saved.updateCheckIntervalHours);
      setIntervalText(String(saved.updateCheckIntervalHours));
      onUpdateSettingsChanged?.(saved.autoCheckUpdate, saved.updateCheckIntervalHours);
    } catch (error: unknown) {
      setAutoCheckUpdate(previousEnabled);
      setIntervalHours(previousHours);
      setIntervalText(String(previousHours));
      setUpdateSettingsError(t("保存自动检测设置失败：{0}", [describeError(error)]));
    } finally {
      setUpdateSettingsBusy(false);
    }
  }

  /** 间隔输入框改动：合法就立即保存，不合法只提示、不写盘。 */
  function handleIntervalChange(raw: string) {
    setIntervalText(raw);
    const trimmed = raw.trim();
    if (!/^\d+$/.test(trimmed)) {
      setUpdateSettingsError(t("检测间隔要填 1 到 168 之间的整数小时。"));
      return;
    }
    const parsed = Number(trimmed);
    if (parsed < UPDATE_INTERVAL_MIN || parsed > UPDATE_INTERVAL_MAX) {
      setUpdateSettingsError(t("检测间隔要填 1 到 168 之间的整数小时。"));
      return;
    }
    setUpdateSettingsError("");
    void saveUpdateSettings(autoCheckUpdate, parsed);
  }

  return (
    <>
      <section className="panel" aria-label={t("版本与更新")}>
        <div className="panel-head">
          <h3 className="settings-subtitle">{t("版本与更新")}</h3>
        </div>

        <dl className="status">
          <dt>{t("当前版本")}</dt>
          <dd>{version ? `Y-Mail ${version}` : t("读取中…")}</dd>
        </dl>
        {versionError && (
          <p className="error" role="alert">
            {t("读版本失败：{0}", [versionError])}
          </p>
        )}

        <div className="switch-rows">
          <label className="switch-row">
            <span className="switch-row-text">
              <span className="switch-row-title">{t("自动检测更新")}</span>
            </span>
            <input
              type="checkbox"
              className="switch-input"
              checked={autoCheckUpdate}
              disabled={updateSettingsBusy}
              onChange={(event) =>
                void saveUpdateSettings(event.target.checked, intervalHours)
              }
            />
          </label>

          <label className="switch-row">
            <span className="switch-row-text">
              <span className="switch-row-title">{t("检测间隔（小时）")}</span>
            </span>
            <input
              type="number"
              className="update-interval-input"
              min={UPDATE_INTERVAL_MIN}
              max={UPDATE_INTERVAL_MAX}
              step={1}
              inputMode="numeric"
              value={intervalText}
              disabled={updateSettingsBusy}
              onChange={(event) => handleIntervalChange(event.target.value)}
            />
          </label>
        </div>
        {updateSettingsError && (
          <p className="error" role="alert">
            {updateSettingsError}
          </p>
        )}

        <div className="actions">
          <button
            type="button"
            onClick={() => void checkUpdate()}
            disabled={state.kind === "checking" || installing}
            aria-busy={state.kind === "checking"}
          >
            {state.kind === "checking" ? t("正在检查更新……") : t("检查更新")}
          </button>
        </div>

        {state.kind === "latest" && (
          <p className="notice" role="status">
            {t("已经是最新版本。")}
          </p>
        )}
        {state.kind === "failed" && (
          <p className="error" role="alert">
            {t("检查更新失败：{0}", [state.message])}
          </p>
        )}
        {state.kind === "found" && (
          <>
            <p className="notice" role="status">
              {t("发现新版本 {0}（当前 {1}）。", [
                state.info.version,
                state.info.currentVersion || version,
              ])}
            </p>
            {state.info.notes && <pre className="update-notes">{state.info.notes}</pre>}
            <div className="actions">
              <button
                type="button"
                onClick={() => void installUpdate()}
                disabled={installing}
                aria-busy={installing}
              >
                {installing ? t("正在下载安装……") : t("下载并安装")}
              </button>
            </div>
            {installing && (
              <p className="hint" role="status">
                {percent === null ? t("正在下载更新……") : t("已下载 {0}%", [percent])}
              </p>
            )}
            {installing && (
              <progress
                className="update-progress"
                aria-label={t("下载进度")}
                max={100}
                value={percent ?? 0}
              />
            )}
            {installError && (
              <p className="error" role="alert">
                {installError}
              </p>
            )}
          </>
        )}
      </section>

      <section className="panel" aria-label={t("项目主页")}>
        <div className="panel-head">
          <h3 className="settings-subtitle">{t("项目主页")}</h3>
        </div>
        <div className="actions">
          <button type="button" onClick={() => void openUrl(GITHUB_HOME)} disabled={opening}>
            {t("打开 GitHub 页面")}
          </button>
          <button type="button" onClick={() => void openUrl(GITHUB_RELEASES)} disabled={opening}>
            {t("打开发布页")}
          </button>
        </div>
        {openError && (
          <p className="error" role="alert">
            {openError}
          </p>
        )}
      </section>
    </>
  );
}