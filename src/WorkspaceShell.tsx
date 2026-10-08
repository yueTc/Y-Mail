import type { ReactNode } from "react";
import { t } from "./i18n";

export type WorkspaceMode = "inbox" | "contacts" | "settings";

type WorkspaceShellProps = {
  mode: WorkspaceMode;
  onModeChange: (mode: WorkspaceMode) => void;
  inbox: ReactNode;
  contacts: ReactNode;
  settings: ReactNode;
};

/**
 * 最左设置栏的入口，顺序就是显示顺序：收件箱在上、设置在下，
 * 通讯录夹在中间。想再加入口先跟用户确认，别自己塞。
 */
const ENTRIES: ReadonlyArray<{ id: WorkspaceMode; label: string }> = [
  { id: "inbox", label: "收件箱" },
  { id: "contacts", label: "通讯录" },
  { id: "settings", label: "设置" },
];

function EntryIcon({ id }: { id: WorkspaceMode }) {
  if (id === "settings") {
    return (
      <svg
        className="rail-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09a1.65 1.65 0 0 0-1.08-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    );
  }
  if (id === "contacts") {
    return (
      <svg
        className="rail-icon"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden="true"
      >
        <path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2" />
        <circle cx="9" cy="7" r="4" />
        <path d="M22 21v-2a4 4 0 0 0-3-3.87" />
        <path d="M16 3.13a4 4 0 0 1 0 7.75" />
      </svg>
    );
  }
  return (
    <svg
      className="rail-icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M22 12h-6l-2 3h-4l-2-3H2" />
      <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
    </svg>
  );
}

/**
 * 工作区外壳：最左固定设置栏 + 右侧内容区。
 * 收件箱、通讯录和单栏设置页同时挂载，靠 hidden 切换，
 * 这样来回切换不会丢掉选中的账号、文件夹、邮件和面板宽度。
 */
export default function WorkspaceShell({
  mode,
  onModeChange,
  inbox,
  contacts,
  settings,
}: WorkspaceShellProps) {
  return (
    <div className="workspace">
      <nav className="workspace-rail" aria-label={t("主导航")}>
        {ENTRIES.map((entry) => (
          <button
            key={entry.id}
            type="button"
            className={mode === entry.id ? "rail-button active" : "rail-button"}
            aria-current={mode === entry.id ? "page" : undefined}
            aria-label={t(entry.label)}
            title={t(entry.label)}
            onClick={() => onModeChange(entry.id)}
          >
            <EntryIcon id={entry.id} />
            <span className="rail-label">{t(entry.label)}</span>
          </button>
        ))}
      </nav>
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