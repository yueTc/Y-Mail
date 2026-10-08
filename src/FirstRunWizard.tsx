import { useCallback, useState, type JSX } from "react";
import { open } from "@tauri-apps/plugin-dialog";

import { api, describeError, type AppSettings } from "./api";
import RestartChoiceDialog from "./RestartChoiceDialog";
import { t } from "./i18n";

export type FirstRunWizardProps = {
  /** 启动时读到的设置；默认位置与通知开关都从这里取初值。 */
  settings: AppSettings;
  /** 用户选完（用默认，或自定义目录设置成功）后回调。 */
  onDone: () => void;
};

/** 抹平斜杠与大小写后比较：判断用户选的目录是不是就是默认目录。 */
function sameDir(left: string, right: string): boolean {
  const normalize = (value: string) =>
    value.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
  return normalize(left) === normalize(right);
}

/**
 * 首次启动向导：让用户先定邮件数据放哪。
 *
 * 只给两条路，且都会写 settings.json，所以向导只会出现这一次：
 * 1. 用默认位置——引擎本来就在默认目录，不用重启；
 * 2. 选择文件夹——复用迁移流程，重启后引擎才开到新目录。
 * 不提供关闭按钮：必须二选一。
 */
export default function FirstRunWizard({ settings, onDone }: FirstRunWizardProps): JSX.Element {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [restartNeeded, setRestartNeeded] = useState(false);

  /** 用默认位置：写一份默认设置就行，不用重启。 */
  const useDefault = useCallback(async () => {
    setError(null);
    setBusy(true);
    try {
      await api.saveAppSettings({
        dataDir: "",
        attachmentDir: "",
        notifyNewMail: settings.notifyNewMail,
      });
      onDone();
    } catch (cause) {
      setError(t("保存默认位置失败：{0}", [describeError(cause)]));
      setBusy(false);
    }
  }, [onDone, settings.notifyNewMail]);

  /** 选文件夹：走迁移（复制 + 校验通过才写设置），完成后要重启。 */
  const chooseFolder = useCallback(async () => {
    setError(null);
    let picked: string | null = null;
    try {
      const chosen = await open({ directory: true, multiple: false, title: t("选择邮件数据目录") });
      picked = typeof chosen === "string" ? chosen : null;
    } catch (cause) {
      setError(t("打开目录选择失败：{0}", [describeError(cause)]));
      return;
    }
    if (picked === null) return; // 用户在系统对话框里点了取消
    if (sameDir(picked, settings.defaultDataDir)) {
      await useDefault();
      return;
    }
    setBusy(true);
    try {
      let result = await api.changeDataDir(picked, false);
      if (result.needsConfirmation) {
        if (!window.confirm(result.message)) return;
        result = await api.changeDataDir(picked, true);
      }
      if (result.needsConfirmation) {
        throw new Error(result.message || t("目标目录需要确认"));
      }
      setRestartNeeded(true);
    } catch (cause) {
      setError(t("设置数据目录失败：{0}", [describeError(cause)]));
    } finally {
      setBusy(false);
    }
  }, [settings.defaultDataDir, useDefault]);

  /** 重启应用，让引擎开到新目录。 */
  const restart = useCallback(async (cleanup: boolean) => {
    setError(null);
    setBusy(true);
    try {
      await api.restartApp(cleanup);
    } catch (cause) {
      setError(t("重启失败，请手动关掉再打开：{0}", [describeError(cause)]));
      setBusy(false);
    }
  }, []);

  if (restartNeeded) {
    return (
      <RestartChoiceDialog
        busy={busy}
        error={error}
        onChoose={(cleanup) => void restart(cleanup)}
      />
    );
  }

  return (
    <div className="modal-backdrop">
      <div
        className="modal first-run"
        role="dialog"
        aria-modal="true"
        aria-labelledby="first-run-title"
      >
        <h2 id="first-run-title">{t("先定一下邮件数据放哪")}</h2>
        <p>
          {t("第一次使用，先挑一个文件夹存邮件数据（数据库、附件下载、日志都放这里）。 以后想换，在「设置」页里也能改。")}</p>
        <p className="first-run-default">
          {t("默认位置：")}<code>{settings.defaultDataDir}</code>
        </p>
        {error !== null && <p className="first-run-error">{error}</p>}
        <div className="actions">
          <button
            type="button"
            className="primary"
            onClick={() => void useDefault()}
            disabled={busy}
          >
            {t("用默认位置")}</button>
          <button type="button" onClick={() => void chooseFolder()} disabled={busy}>
            {t("选择文件夹…")}</button>
        </div>
      </div>
    </div>
  );
}