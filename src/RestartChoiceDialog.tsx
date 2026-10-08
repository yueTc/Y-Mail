import { t } from "./i18n";
type RestartChoiceDialogProps = {
  /** 正在回调重启命令时禁用按钮，避免连点。 */
  busy: boolean;
  /** 重启命令失败时可读原因；成功后应用会直接重启。 */
  error: string | null;
  /** 用户选择是否清理旧数据目录。 */
  onChoose: (cleanup: boolean) => void;
};

/**
 * 迁移完成后的重启选择弹窗。
 *
 * 这个弹窗故意不能关闭：用户必须明确选「清理」或「不清理」，否则应用会停在
 * 一个已经切换设置、但引擎还没切目录的中间状态。
 */
export default function RestartChoiceDialog({
  busy,
  error,
  onChoose,
}: RestartChoiceDialogProps) {
  return (
    <div className="modal-backdrop">
      <div
        className="modal first-run"
        role="dialog"
        aria-modal="true"
        aria-labelledby="restart-choice-title"
        aria-describedby="restart-choice-desc"
      >
        <h2 id="restart-choice-title">{t("数据目录已经切换")}</h2>
        <p id="restart-choice-desc">
          {t("需要重启应用后才会使用新目录。重启时当前所有窗口都会关闭。")}</p>
        <p className="hint">
          {t("是否清理旧数据目录里的邮件数据？旧目录里的 settings.json 会保留， 只清数据库、日志、下载、配图和备份。")}</p>
        {error !== null && <p className="first-run-error">{error}</p>}
        <div className="actions">
          <button
            type="button"
            className="primary"
            onClick={() => onChoose(true)}
            disabled={busy}
          >
            {t("清理并重启")}</button>
          <button type="button" onClick={() => onChoose(false)} disabled={busy}>
            {t("直接重启（不清理旧文件）")}</button>
        </div>
      </div>
    </div>
  );
}