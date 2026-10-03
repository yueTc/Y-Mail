import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** 与 Rust 侧 `DbStatus` 一一对应（camelCase）。 */
interface DbStatus {
  databaseFile: string;
  logDir: string;
  schemaVersion: number;
  appliedCount: number;
  appliedVersions: number[];
  fts5Available: boolean;
}

type LoadState =
  | { kind: "loading" }
  | { kind: "ready"; status: DbStatus }
  | { kind: "error"; message: string };

export default function App() {
  const [state, setState] = useState<LoadState>({ kind: "loading" });

  useEffect(() => {
    let cancelled = false;

    invoke<DbStatus>("db_status")
      .then((status) => {
        if (!cancelled) setState({ kind: "ready", status });
      })
      .catch((error: unknown) => {
        if (!cancelled) setState({ kind: "error", message: String(error) });
      });

    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <main className="shell">
      <h1>统一收件箱（骨架）</h1>
      <p className="subtitle">Wave 0：外壳、数据库与迁移链路已就位，账号与邮件功能尚未实现。</p>

      <section className="panel">
        <h2>数据库状态</h2>
        {state.kind === "loading" && <p className="hint">正在读取……</p>}
        {state.kind === "error" && (
          <p className="error">读取失败：{state.message}</p>
        )}
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
    </main>
  );
}