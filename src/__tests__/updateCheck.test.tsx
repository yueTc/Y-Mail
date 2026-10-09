//! 自动检测更新回归：启动满 30 秒首查、按小时循环、失败静默、同一版本只提醒一次。
//!
//! 定时器跑在最外层 `App.tsx`，所以这里把几个重面板换成空壳，只留调度和红点这条线。

import type { ReactNode } from "react";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import App from "../App";
import { api, type AppSettings } from "../api";
import { UPDATE_NOTIFIED_VERSION_KEY } from "../updateNotice";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getAppSettings: vi.fn(),
    checkForUpdate: vi.fn(),
  },
}));

// 只留设置页这条线；其它重面板换空壳，免得拖进网络和编辑器。
vi.mock("../InboxPanel", () => ({ default: () => null }));
vi.mock("../ContactsWorkspace", () => ({ default: () => null }));
vi.mock("../FirstRunWizard", () => ({ default: () => null }));
vi.mock("../WorkspaceShell", () => ({
  default: (props: { settings: ReactNode }) => <>{props.settings}</>,
}));
vi.mock("../SettingsWorkspace", () => ({
  default: (props: {
    hasUpdateNotice?: boolean;
    onAboutOpened?: () => void;
    onUpdateFound?: (version: string) => void;
  }) => (
    <div>
      <span data-testid="notice">{props.hasUpdateNotice ? "有" : "无"}</span>
      <button type="button" onClick={() => props.onAboutOpened?.()}>
        open-about
      </button>
      <button type="button" onClick={() => props.onUpdateFound?.("0.1.3")}>
        manual-found
      </button>
    </div>
  ),
}));

const BASE_SETTINGS: AppSettings = {
  dataDir: "",
  attachmentDir: "",
  notifyNewMail: true,
  notifyAiEnabled: false,
  blockRemoteImagesByDefault: true,
  minimizeToTrayOnClose: true,
  startMinimizedToTray: false,
  autoCheckUpdate: true,
  updateCheckIntervalHours: 1,
  defaultDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  defaultAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  activeDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  activeAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  firstRun: false,
};

const UPDATE_INFO = {
  version: "0.1.3",
  currentVersion: "0.1.2",
  notes: "修了几个问题",
  date: "2026-10-08T00:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.useFakeTimers();
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

/** 等设置读进来、定时器排上。 */
async function settle() {
  await act(async () => {
    await Promise.resolve();
  });
}

describe("自动检测更新调度", () => {
  it("开关打开后启动满 30 秒首查一次，失败静默不弹错", async () => {
    vi.mocked(api.getAppSettings).mockResolvedValue(BASE_SETTINGS);
    vi.mocked(api.checkForUpdate).mockRejectedValue(new Error("连不上更新服务器"));
    render(<App />);
    await settle();

    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    await settle();

    expect(api.checkForUpdate).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("开关关着就不检查", async () => {
    vi.mocked(api.getAppSettings).mockResolvedValue({
      ...BASE_SETTINGS,
      autoCheckUpdate: false,
    });
    vi.mocked(api.checkForUpdate).mockResolvedValue(null);
    render(<App />);
    await settle();

    await act(async () => {
      vi.advanceTimersByTime(60 * 60 * 1000);
    });
    await settle();

    expect(api.checkForUpdate).not.toHaveBeenCalled();
  });

  it("查到新版本点亮红点，点开关于清掉，同一版本不再提醒", async () => {
    vi.mocked(api.getAppSettings).mockResolvedValue(BASE_SETTINGS);
    vi.mocked(api.checkForUpdate).mockResolvedValue(UPDATE_INFO);
    render(<App />);
    await settle();

    expect(screen.getByTestId("notice").textContent).toBe("无");

    await act(async () => {
      vi.advanceTimersByTime(30_000);
    });
    await settle();
    expect(api.checkForUpdate).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("notice").textContent).toBe("有");

    fireEvent.click(screen.getByRole("button", { name: "open-about" }));
    expect(screen.getByTestId("notice").textContent).toBe("无");
    expect(window.localStorage.getItem(UPDATE_NOTIFIED_VERSION_KEY)).toBe("0.1.3");

    // 过了一个间隔又查到同一个版本：已经提醒过，不再点亮。
    await act(async () => {
      vi.advanceTimersByTime(60 * 60 * 1000);
    });
    await settle();
    expect(api.checkForUpdate).toHaveBeenCalledTimes(2);
    expect(screen.getByTestId("notice").textContent).toBe("无");
  });

  it("手动检查查到新版本也会点亮红点", async () => {
    vi.mocked(api.getAppSettings).mockResolvedValue({
      ...BASE_SETTINGS,
      autoCheckUpdate: false,
    });
    render(<App />);
    await settle();

    fireEvent.click(screen.getByRole("button", { name: "manual-found" }));

    expect(screen.getByTestId("notice").textContent).toBe("有");
  });
});
