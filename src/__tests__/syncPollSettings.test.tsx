//! 「拉信间隔」设置回归：面板位置、范围校验、保存成功、保存失败回滚。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SettingsWorkspace from "../SettingsWorkspace";
import { api, type AppSettings } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    getAppSettings: vi.fn(),
    saveAppSettings: vi.fn(),
    changeDataDir: vi.fn(),
    restartApp: vi.fn(),
    openDataDir: vi.fn(),
    clearAutoContacts: vi.fn(),
    autostartStatus: vi.fn(),
    setAutostart: vi.fn(),
    setTraySettings: vi.fn(),
    setUpdateSettings: vi.fn(),
    setSyncPollSettings: vi.fn(),
    appVersion: vi.fn().mockResolvedValue("0.1.2"),
    checkForUpdate: vi.fn(),
    installPendingUpdate: vi.fn(),
    relaunchApp: vi.fn(),
    openExternalUrl: vi.fn(),
  },
}));

// 用一个带标记的同步状态面板，验证「拉信间隔」排在它前面。
vi.mock("../SyncPanel", () => ({
  default: () => <section aria-label="同步状态">同步状态标记</section>,
}));
vi.mock("../AccountPanel", () => ({ default: () => null }));
vi.mock("../ProxyPanel", () => ({ default: () => null }));
vi.mock("../ReaderSettingsPanel", () => ({ default: () => null }));
vi.mock("../AppearanceSettingsPanel", () => ({ default: () => null }));
vi.mock("../AiPanel", () => ({ default: () => null }));
vi.mock("../McpPanel", () => ({ default: () => null }));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const SETTINGS: AppSettings = {
  dataDir: "",
  attachmentDir: "",
  notifyNewMail: true,
  notifyAiEnabled: false,
  blockRemoteImagesByDefault: true,
  minimizeToTrayOnClose: true,
  startMinimizedToTray: false,
  autoCheckUpdate: false,
  updateCheckIntervalHours: 24,
  syncPollIntervalSeconds: 60,
  defaultDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  defaultAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  activeDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  activeAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  firstRun: false,
};

function renderSettings() {
  return render(<SettingsWorkspace proxiesVersion={1} onProxiesChanged={() => {}} />);
}

async function openAccounts() {
  fireEvent.click(await screen.findByRole("button", { name: "账号与同步" }));
}

/** 拉信间隔输入框。 */
async function intervalInput() {
  return (await screen.findByLabelText("拉信间隔（秒）")) as HTMLInputElement;
}

beforeEach(() => {
  vi.mocked(api.getAppSettings).mockResolvedValue(SETTINGS);
  vi.mocked(api.autostartStatus).mockResolvedValue(false);
  vi.mocked(api.setSyncPollSettings).mockImplementation(async (seconds) => ({
    ...SETTINGS,
    syncPollIntervalSeconds: seconds,
  }));
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("设置页拉信间隔", () => {
  it("面板排在同步状态之前，默认显示 60", async () => {
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    expect(interval.value).toBe("60");

    const heading = screen.getByRole("heading", { name: "拉信间隔" });
    const syncStatus = screen.getByLabelText("同步状态");
    // 用文档位置比较：拉信间隔必须出现在同步状态之前。
    const relation = heading.compareDocumentPosition(syncStatus);
    expect(relation & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("填 10 和 3600 都能保存", async () => {
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    fireEvent.change(interval, { target: { value: "10" } });
    await waitFor(() => expect(api.setSyncPollSettings).toHaveBeenCalledWith(10));

    fireEvent.change(interval, { target: { value: "3600" } });
    await waitFor(() => expect(api.setSyncPollSettings).toHaveBeenCalledWith(3600));
  });

  it("填 9 或 3601 当场提示且不保存", async () => {
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    fireEvent.change(interval, { target: { value: "9" } });
    expect(await screen.findByText("拉信间隔要填 10 到 3600 之间的整数秒。")).toBeTruthy();

    fireEvent.change(interval, { target: { value: "3601" } });
    expect(await screen.findByText("拉信间隔要填 10 到 3600 之间的整数秒。")).toBeTruthy();
    expect(api.setSyncPollSettings).not.toHaveBeenCalled();
  });

  it("填空值或非整数当场提示且不保存", async () => {
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    fireEvent.change(interval, { target: { value: "" } });
    expect(await screen.findByText("拉信间隔要填 10 到 3600 之间的整数秒。")).toBeTruthy();

    fireEvent.change(interval, { target: { value: "abc" } });
    expect(await screen.findByText("拉信间隔要填 10 到 3600 之间的整数秒。")).toBeTruthy();
    expect(api.setSyncPollSettings).not.toHaveBeenCalled();
  });

  it("保存成功后给出独立提示", async () => {
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    fireEvent.change(interval, { target: { value: "120" } });

    expect(await screen.findByText("拉信间隔已保存，改完立刻生效。")).toBeTruthy();
    expect(interval.value).toBe("120");
  });

  it("保存失败回滚到改动前的值并提示", async () => {
    vi.mocked(api.setSyncPollSettings).mockRejectedValue(new Error("磁盘满了"));
    renderSettings();
    await openAccounts();

    const interval = await intervalInput();
    fireEvent.change(interval, { target: { value: "120" } });

    expect(await screen.findByText("保存拉信间隔失败：磁盘满了")).toBeTruthy();
    expect(interval.value).toBe("60");
  });
});