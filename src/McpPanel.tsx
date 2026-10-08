//! MCP 外部接入面板（Wave 8）。
//!
//! 只做三件事：显示开关状态（默认「MCP 未启用」）、给一键开关与写工具开关、
//! 展示工具清单与审计。界面不接触凭据，也不显示正文。

import { useCallback, useEffect, useState } from "react";

import { api, describeError, type McpAudit, type McpStatus, type McpTool } from "./api";
import { t } from "./i18n";

/** 状态中文名。 */
function auditStatusLabel(status: string): string {
  switch (status) {
    case "ok":
      return t("成功");
    case "rejected":
      return t("被拒");
    case "error":
      return t("出错");
    default:
      return status;
  }
}

export default function McpPanel() {
  const [status, setStatus] = useState<McpStatus>();
  const [tools, setTools] = useState<McpTool[]>([]);
  const [audit, setAudit] = useState<McpAudit[]>([]);
  const [auditTotal, setAuditTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextTools, nextAudit] = await Promise.all([
        api.mcpStatus(),
        api.mcpTools(),
        api.mcpAudit(50),
      ]);
      setStatus(nextStatus);
      setTools(nextTools);
      setAudit(nextAudit.items);
      setAuditTotal(nextAudit.total);
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

  /** 一键开关：总开关或写工具开关。 */
  const toggle = useCallback(
    async (action: "enable" | "write") => {
      if (!status) return;
      setBusy(true);
      try {
        if (action === "enable") {
          const next = await api.mcpSetEnabled(!status.enabled);
          setStatus(next);
        } else {
          const next = await api.mcpSetWriteTools(!status.writeToolsEnabled);
          setStatus(next);
        }
        await refresh();
        setError(undefined);
      } catch (caught) {
        setError(describeError(caught));
      } finally {
        setBusy(false);
      }
    },
    [refresh, status],
  );

  if (loading) {
    return (
      <section className="panel" aria-busy="true">
        <h2>{t("MCP 外部接入")}</h2>
        <p className="hint" role="status">{t("正在读取……")}</p>
      </section>
    );
  }

  return (
    <section className="panel" aria-busy={busy}>
      <div className="panel-head">
        <h2>{t("MCP 外部接入")}</h2>
        <button type="button" aria-busy={busy} onClick={() => void refresh()} disabled={busy}>
          {t("刷新")}</button>
      </div>

      {error && <p className="error" role="alert">{t("操作失败：")}{error}</p>}

      {status && !status.enabled && (
        <p className="hint">
          <strong>{t("MCP 未启用。")}</strong>
          {t("外部 Agent（Codex / Claude Desktop / Cursor 等）现在连不上本机邮箱。要用的话， 先点下面的「启用 MCP」，再把配置示例填进对应 Agent 的配置文件。")}</p>
      )}

      {status?.enabled && (
        <p className="hint">
          {t("MCP 已启用（只走本地标准输入输出，不监听任何网络端口）。外部 Agent 需要用配置文件 拉起本机的 ")}
          {status.binaryName}{t(" 程序。")}
        </p>
      )}

      {status && (
        <>
          <div className="panel-head">
            <div>
              <button
                type="button"
                className={status.enabled ? "danger" : "primary"}
                onClick={() => void toggle("enable")}
                aria-busy={busy}
                disabled={busy}
              >
                {status.enabled ? t("一键关闭 MCP") : t("启用 MCP")}
              </button>
            </div>
            <span className="hint" role="status">{status.enabled ? t("当前：已启用") : t("当前：未启用")}</span>
          </div>

          <h3>{t("写工具（默认只读）")}</h3>
          <p className="hint">
            {t("只读工具任何时候都可用；写工具指「建草稿」。默认关闭。打开后外部 Agent 只能往草稿箱里 存草稿，")}<strong>{t("不会发送")}</strong>{t("，发送必须由你在应用里手动确认。风险：Agent 可能被邮件正文里的 诱导内容带偏，生成你不需要的草稿，请自己核对后再发。")}</p>
          <div className="panel-head">
            <button
              type="button"
              className={status.writeToolsEnabled ? "danger" : undefined}
              onClick={() => void toggle("write")}
              aria-busy={busy}
              disabled={busy || !status.enabled}
            >
              {status.writeToolsEnabled ? t("关闭写工具") : t("打开写工具")}
            </button>
            <span className="hint" role="status">
              {status.enabled
                ? status.writeToolsEnabled
                  ? t("当前：允许建草稿")
                  : t("当前：只读")
                : t("总开关关着，写工具不可用")}
            </span>
          </div>

          <h3>{t("工具清单")}</h3>
          <ul className="card-list">
            {tools.map((tool) => (
              <li className="card" key={tool.name}>
                <div>
                  <strong>{tool.title}</strong>
                  <code className="path">{tool.name}</code>
                  <p className="hint">{tool.description}</p>
                </div>
                <span className="hint">
                  {tool.readOnly ? t("只读") : t("写")}
                  {tool.enabled ? "" : t("（当前不可用）")}
                </span>
              </li>
            ))}
          </ul>
          <p className="hint">{t("清单里没有发送类、导出类工具；邮件正文按不可信输入处理并截断后返回。")}</p>

          <h3>{t("外部 Agent 配置说明")}</h3>
          <p className="hint">
            {t("安装包已自带 ")}{status.binaryName}{t(".exe，它和主程序装在同一目录，不用手填路径。 下面是按本机安装位置生成的配置（")}<span className="path">{status.binaryPath}</span>{t("）， 直接粘进 Agent 的配置文件即可。数据目录环境变量名是")}<code>{status.dataDirEnv}</code>{t("，本机数据目录：")}<span className="path">{status.dataDir}</span>{t("。 支持的协议版本：")}{status.protocolVersions.join(t("、"))}{t("。常见安装位置：按用户安装是")}<code>%LOCALAPPDATA%\Y-Mail</code>{t("，机器级安装是")}<code>C:\Program Files\Y-Mail</code>{t("，可执行文件都在主程序同目录。")}</p>
          <pre className="path">{status.configExample}</pre>

          <h3>{t("审计（共 ")}{auditTotal}{t(" 条）")}</h3>
          {audit.length === 0 ? (
            <p className="hint">{t("还没有调用记录。")}</p>
          ) : (
            <ul className="card-list">
              {audit.map((row) => (
                <li className="card" key={row.id}>
                  <div>
                    <strong>{row.tool}</strong>
                    <p className="hint">
                      {auditStatusLabel(row.status)}
                      {t("｜账号范围 ")}
                      {row.accountScope}{t("｜")}{row.ts}
                    </p>
                    <p className="hint">
                      {t("参数摘要（哈希，不含原文）：")}<code>{row.argsDigest.slice(0, 16)}…</code>
                    </p>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </>
      )}
    </section>
  );
}