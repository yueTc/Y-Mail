import type { ReactNode } from "react";

export type WorkspaceMode = "inbox" | "contacts" | "settings";

type WorkspaceShellProps = {
  mode: WorkspaceMode;
  inbox: ReactNode;
  contacts: ReactNode;
  settings: ReactNode;
};

/**
 * 工作区外壳只负责保留三个页面的挂载状态。
 * 页面各自的标题栏、返回入口和账号入口都在页面内部，避免再出现最左导航栏。
 */
export default function WorkspaceShell({
  mode,
  inbox,
  contacts,
  settings,
}: WorkspaceShellProps) {
  return (
    <div className="workspace">
      <div className="workspace-body">
        <div className="workspace-pane" hidden={mode !== "inbox"}>
          {inbox}
        </div>
        <div className="workspace-pane" hidden={mode !== "contacts"}>
          {contacts}
        </div>
        <div className="workspace-pane" hidden={mode !== "settings"}>
          {settings}
        </div>
      </div>
    </div>
  );
}
