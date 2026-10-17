import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";

import appIcon from "../src-tauri/icons/128x128.png";
import { t } from "./i18n";

type AppTitleBarProps = {
  /** 左侧固定区域，例如品牌或返回按钮。 */
  leading: ReactNode;
  /** 标题栏中间区域，用于和邮件列表栏对齐的搜索框。 */
  center?: ReactNode;
  /** 中间区域宽度；传入列表栏宽度后，搜索框会与下方栏位保持同一条竖线。 */
  centerWidth?: number;
  /** 最小化按钮前方的操作按钮。 */
  actions?: ReactNode;
  /** 左侧区域宽度；收件箱传入邮箱栏当前宽度，保证品牌和栏位对齐。 */
  leadingWidth?: number;
  className?: string;
};

/** 应用品牌：沿用安装包图标，不再另画一套标志。 */
export function AppBrand() {
  return (
    <div className="app-brand" data-tauri-drag-region>
      <img className="app-brand-icon" src={appIcon} alt="" width={24} height={24} />
      <span className="app-brand-name">Y-Mail</span>
    </div>
  );
}

/** 通讯录和设置页共用的返回入口。 */
export function BackToInboxButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      className="titlebar-back"
      aria-label={t("返回收件箱")}
      title={t("返回收件箱")}
      onClick={onClick}
    >
      <svg viewBox="0 0 20 20" aria-hidden="true" focusable="false">
        <path d="m11.5 4.5-5 5 5 5M7 9.5h7" />
      </svg>
      <span>{t("收件箱")}</span>
    </button>
  );
}

function windowAction(event: ReactMouseEvent<HTMLElement>): void {
  event.preventDefault();
  event.stopPropagation();
}

/** 自定义窗口按钮；浏览器预览时没有 Tauri 窗口，不渲染空按钮。 */
function WindowControls() {
  if (!isTauri()) return null;
  const currentWindow = getCurrentWindow();

  return (
    <div className="window-controls" aria-label={t("窗口控制")}>
      <button
        type="button"
        className="window-control-button"
        aria-label={t("最小化")}
        title={t("最小化")}
        onClick={() => void currentWindow.minimize()}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="M3 8.5h10" />
        </svg>
      </button>
      <button
        type="button"
        className="window-control-button"
        aria-label={t("最大化或还原")}
        title={t("最大化或还原")}
        onClick={() => void currentWindow.toggleMaximize()}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <rect x="3.5" y="3.5" width="9" height="9" />
        </svg>
      </button>
      <button
        type="button"
        className="window-control-button window-control-close"
        aria-label={t("关闭")}
        title={t("关闭")}
        onClick={() => void currentWindow.close()}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true" focusable="false">
          <path d="m4 4 8 8M12 4l-8 8" />
        </svg>
      </button>
    </div>
  );
}

/**
 * 应用自己的窗口栏。系统标题栏关闭后，这里负责拖动和窗口按钮；
 * 操作按钮仍保持普通点击语义，不参与拖动。
 */
export default function AppTitleBar({
  leading,
  center,
  centerWidth,
  actions,
  leadingWidth,
  className = "",
}: AppTitleBarProps) {
  const toggleMaximize = (event: ReactMouseEvent<HTMLElement>) => {
    if (!isTauri()) return;
    if (event.target instanceof Element && event.target.closest("button, a, input, select, textarea")) {
      return;
    }
    void getCurrentWindow().toggleMaximize();
  };

  return (
    <header
      className={className ? `app-titlebar ${className}` : "app-titlebar"}
      onDoubleClick={toggleMaximize}
    >
      <div
        className="app-titlebar-leading"
        data-tauri-drag-region
        style={leadingWidth === undefined ? undefined : { width: leadingWidth }}
      >
        {leading}
      </div>
      {center !== undefined && (
        <div
          className="app-titlebar-center"
          data-tauri-drag-region
          style={centerWidth === undefined ? undefined : { width: centerWidth }}
        >
          {center}
        </div>
      )}
      <div className="app-titlebar-drag" data-tauri-drag-region aria-hidden="true" />
      <div className="app-titlebar-actions" onMouseDown={windowAction}>
        {actions}
        <WindowControls />
      </div>
    </header>
  );
}