//! 关于：看当前版本、打开项目主页、查更新并就地热更新。
//!
//! 更新走 Tauri 官方更新插件：更新包带发布方签名，下载后先验签再安装，装完重启。
//! 应用里只放公钥，改不了更新包；网址写死在本文件，界面传不进任意链接。

import { useEffect, useState } from "react";

import { api, describeError, type UpdateInfo } from "./api";
import { t } from "./i18n";

/** 项目主页（源码、问题反馈）。 */
const GITHUB_HOME = "https://github.com/yueTc/Y-Mail";
/** 下载页：指向最新一次发布。 */
const GITHUB_RELEASES = "https://github.com/yueTc/Y-Mail/releases/latest";

/** 查更新这一步的四种状态；分开写是为了让界面只显示该显示的那句。 */
type CheckState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "found"; info: UpdateInfo }
  | { kind: "failed"; message: string };

export default function AboutPanel() {
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
      setState(info ? { kind: "found", info } : { kind: "latest" });
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
        <p className="hint">{t("源码、问题反馈和每次发布的更新说明都在这里。")}</p>
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