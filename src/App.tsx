import { useEffect, useState } from "react";

import { api, describeError, type DbStatus } from "./api";
import AccountPanel from "./AccountPanel";
import AiPanel from "./AiPanel";
import InboxPanel from "./InboxPanel";
import McpPanel from "./McpPanel";
import ProxyPanel from "./ProxyPanel";
import SyncPanel from "./SyncPanel";

type LoadState =
  | { kind: "loading" }
  | { kind: "ready"; status: DbStatus }
  | { kind: "error"; message: string };

/** 顶部标签页：收件箱是主界面，配置类面板收在第二个页签里。 */
type Tab = "inbox" | "settings";

export default function App() {
  const [tab, setTab] = useState<Tab>("inbox");
  const [state, setState] = useState<LoadState>({ kind: "loading" });
  // 代理列表变过一次就加一，用来通知账号面板重新拉取「指定代理」的可选项。
  const [proxiesVersion, setProxiesVersion] = useState(0);

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

  return (
    <main className="shell">
      <header className="app-head">
        <h1>统一收件箱</h1>
        <nav className="tabs" aria-label="主导航">
          <button
            type="button"
            className={tab === "inbox" ? "tab active" : "tab"}
            onClick={() => setTab("inbox")}
          >
            收件箱
          </button>
          <button
            type="button"
            className={tab === "settings" ? "tab active" : "tab"}
            onClick={() => setTab("settings")}
          >
            账号与代理
          </button>
        </nav>
      </header>

      {tab === "inbox" && <InboxPanel />}

      {tab === "settings" && (
        <>
          <p className="subtitle">
            授权码只进 Windows 凭据管理器，保存前会先做一次连接自检。
          </p>

          <SyncPanel />

          <AccountPanel proxiesVersion={proxiesVersion} />

          <ProxyPanel onChanged={() => setProxiesVersion((value) => value + 1)} />

          <AiPanel />

          <section className="panel">
            <h2>数据库状态</h2>
            {state.kind === "loading" && <p className="hint">正在读取……</p>}
            {state.kind === "error" && <p className="error">读取失败：{state.message}</p>}
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
                <dt>日志目录</dt>
                <dd className="path">{state.status.logDir}</dd>
              </dl>
            )}
          </section>

          <McpPanel />
        </>
      )}
    </main>
  );
}