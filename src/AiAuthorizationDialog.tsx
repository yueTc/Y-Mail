//! AI 外发授权弹窗（Wave 7）。
//!
//! 真正发起模型调用前必须经过这里。弹窗只展示后端预览返回的
//! 域名、模型和是否本地，不展示邮件正文，也不写入任何存储。

import type { AiAuthorization, AiFunction } from "./api";

const FUNCTION_LABEL: Record<AiFunction, string> = {
  translate: "翻译",
  summary: "摘要",
  polish: "润色",
  draft: "起草",
};

export interface AiAuthorizationDialogProps {
  preview: AiAuthorization;
  busy?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}

/** 通用的 AI 调用确认框。 */
export default function AiAuthorizationDialog({
  preview,
  busy = false,
  onCancel,
  onConfirm,
}: AiAuthorizationDialogProps) {
  return (
    <div className="ai-modal-backdrop" role="presentation">
      <section
        className="ai-modal"
        role="dialog"
        aria-modal="true"
        aria-label="确认 AI 外发"
      >
        <h3>确认 AI 调用</h3>
        <p>
          这次要执行「{FUNCTION_LABEL[preview.function as AiFunction] ?? preview.function}」，
          请确认目标后再发送。
        </p>
        <dl className="ai-modal-targets">
          <dt>要发给哪个域名</dt>
          <dd>{preview.host}</dd>
          <dt>用哪个模型</dt>
          <dd>{preview.model}</dd>
          <dt>是不是本地服务</dt>
          <dd>{preview.local ? "是，本地服务，内容不离开这台电脑" : "不是，内容会发送到远程站点"}</dd>
          <dt>站点</dt>
          <dd>{preview.providerLabel}</dd>
        </dl>
        <p className="hint">
          确认后这次授权在 {preview.expiresInSeconds} 秒内有效，并且只能用一次。
        </p>
        <div className="form-actions">
          <button type="button" className="primary" onClick={onConfirm} disabled={busy}>
            {busy ? "正在调用……" : "确认并调用"}
          </button>
          <button type="button" onClick={onCancel} disabled={busy}>
            取消
          </button>
        </div>
      </section>
    </div>
  );
}