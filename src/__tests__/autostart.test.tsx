//! 设置页开机启动开关回归：默认状态、勾选、取消、失败回退。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SettingsWorkspace from "../SettingsWorkspace";
import { api, type AppSettings, type DbStatus } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    dbStatus: vi.fn(),
    getAppSettings: vi.fn(),
    saveAppSettings: vi.fn(),
    changeDataDir: vi.fn(),
    openDataDir: vi.fn(),
    clearAutoContacts: vi.fn(),
    autostartStatus: vi.fn(),
    setAutostart: vi.fn(),
  },
}));

// 设置页复用了其它面板；这里只测启动分组，把它们替换成空壳。
vi.mock("../SyncPanel", () => ({ default: () => null }));
vi.mock("../AccountPanel", () => ({ default: () => null }));
vi.mock("../ProxyPanel", () => ({ default: () => null }));
vi.mock("../ReaderSettingsPanel", () => ({ default: () => null }));
vi.mock("../AppearanceSettingsPanel", () => ({ default: () => null }));
vi.mock("../AiPanel", () => ({ default: () => null }));
vi.mock("../McpPanel", () => ({ default: () => null }));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
}));

const SETTINGS: AppSettings = {
  dataDir: "",
  attachmentDir: "",
  notifyNewMail: true,
  defaultDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  defaultAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  activeDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  activeAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  firstRun: false,
};

const STATUS: DbStatus = {
  databaseFile: "C:/db/ymail.db",
  logDir: "C:/db/logs",
  attachmentDir: "C:/db/downloads",
  schemaVersion: 9,
  appliedCount: 9,
  appliedVersions: [1, 2],
  fts5Available: true,
};

function renderSettings() {
  return render(
    <SettingsWorkspace
      proxiesVersion={1}
      onProxiesChanged={() => {}}
      onGoInbox={() => {}}
    />,
  );
}

/** 「开机自动启动」这个复选框。 */
function autostartBox() {
  return screen.getByRole("checkbox", { name: /开机自动启动/ }) as HTMLInputElement;
}

describe("设置页开机启动", () => {
  beforeEach(() => {
    vi.mocked(api.dbStatus).mockResolvedValue(STATUS);
    vi.mocked(api.getAppSettings).mockResolvedValue(SETTINGS);
    vi.mocked(api.autostartStatus).mockResolvedValue(false);
    vi.mocked(api.setAutostart).mockResolvedValue(false);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("勾选状态等于后端真实状态", async () => {
    vi.mocked(api.autostartStatus).mockResolvedValue(true);

    renderSettings();

    await waitFor(() => expect(autostartBox().checked).toBe(true));
    expect(api.autostartStatus).toHaveBeenCalledTimes(1);
  });

  it("后端说没开时默认不勾选", async () => {
    renderSettings();

    await waitFor(() => expect(api.autostartStatus).toHaveBeenCalledTimes(1));
    expect(autostartBox().checked).toBe(false);
  });

  it("勾选后调后端，并以后端返回的状态为准", async () => {
    vi.mocked(api.setAutostart).mockResolvedValue(true);
    renderSettings();
    await waitFor(() => expect(api.autostartStatus).toHaveBeenCalledTimes(1));

    fireEvent.click(autostartBox());

    await waitFor(() => expect(api.setAutostart).toHaveBeenCalledWith(true));
    await waitFor(() => expect(autostartBox().checked).toBe(true));
  });

  it("取消勾选后调后端，并回到未勾选", async () => {
    vi.mocked(api.autostartStatus).mockResolvedValue(true);
    renderSettings();
    await waitFor(() => expect(autostartBox().checked).toBe(true));

    fireEvent.click(autostartBox());

    await waitFor(() => expect(api.setAutostart).toHaveBeenCalledWith(false));
    await waitFor(() => expect(autostartBox().checked).toBe(false));
  });

  it("失败时显示错误，并回读真实状态回退", async () => {
    vi.mocked(api.setAutostart).mockRejectedValue(new Error("写启动项被拒绝"));
    renderSettings();
    await waitFor(() => expect(api.autostartStatus).toHaveBeenCalledTimes(1));

    fireEvent.click(autostartBox());

    expect(await screen.findByText(/开机启动设置失败：写启动项被拒绝/)).toBeTruthy();
    // 失败后再回读一次真实状态（第一次是进页面时读的）。
    await waitFor(() => expect(api.autostartStatus).toHaveBeenCalledTimes(2));
    expect(autostartBox().checked).toBe(false);
  });
});
