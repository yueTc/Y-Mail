//! 设置页存储目录与通知开关回归：单目录输入、迁移确认、打开目录。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import SettingsWorkspace from "../SettingsWorkspace";
import { api, type AppSettings, type DbStatus } from "../api";
import { open } from "@tauri-apps/plugin-dialog";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    dbStatus: vi.fn(),
    getAppSettings: vi.fn(),
    saveAppSettings: vi.fn(),
    changeDataDir: vi.fn(),
    restartApp: vi.fn(),
    openDataDir: vi.fn(),
    clearAutoContacts: vi.fn(),
    autostartStatus: vi.fn(),
    setAutostart: vi.fn(),
  },
}));

// 设置页复用了其它面板；这里只测存储分组，把它们替换成空壳。
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

const NEW_SETTINGS: AppSettings = {
  ...SETTINGS,
  dataDir: "D:/Mail",
  defaultAttachmentDir: "D:/Mail/downloads",
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

describe("设置页存储目录与通知", () => {
  beforeEach(() => {
    vi.mocked(api.dbStatus).mockResolvedValue(STATUS);
    vi.mocked(api.getAppSettings).mockResolvedValue(SETTINGS);
    vi.mocked(api.saveAppSettings).mockResolvedValue(SETTINGS);
    vi.mocked(api.changeDataDir).mockResolvedValue({
      needsConfirmation: false,
      message: "目录已切换，请选择是否清理旧文件并重启",
    });
    vi.mocked(api.restartApp).mockResolvedValue(undefined);
    vi.mocked(api.openDataDir).mockResolvedValue(undefined);
    vi.mocked(api.autostartStatus).mockResolvedValue(false);
    vi.mocked(api.setAutostart).mockResolvedValue(false);
    vi.mocked(open).mockResolvedValue("D:/Mail");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("只显示一个可编辑目录输入框，并只读展示下载目录", async () => {
    renderSettings();

    const inputs = await screen.findAllByRole("textbox");
    expect(inputs).toHaveLength(1);
    expect((inputs[0] as HTMLInputElement).value).toBe("");
    expect(screen.getByText(/下载文件保存在：/)).toBeTruthy();
    expect(screen.getAllByText(SETTINGS.defaultAttachmentDir).length).toBeGreaterThan(0);
    expect(
      screen.getByRole("checkbox", { name: /新邮件用 Windows 系统通知提醒/ }),
    ).toBeTruthy();
    expect(screen.getByText(/新邮件用 Windows 系统通知提醒/)).toBeTruthy();
    expect(screen.getByText(SETTINGS.activeDataDir)).toBeTruthy();
  });

  it("保存时只提交通知开关，不再保存单独附件目录", async () => {
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: "保存通知设置" }));

    await waitFor(() =>
      expect(api.saveAppSettings).toHaveBeenCalledWith({
        dataDir: "",
        attachmentDir: "",
        notifyNewMail: true,
      }),
    );
    expect(await screen.findByText("通知设置已保存。")).toBeTruthy();
  });

  it("目标已有数据时先确认，再提交 confirmed=true 并提示重启", async () => {
    vi.mocked(api.getAppSettings)
      .mockResolvedValueOnce(SETTINGS)
      .mockResolvedValueOnce(NEW_SETTINGS);
    vi.mocked(api.changeDataDir)
      .mockResolvedValueOnce({ needsConfirmation: true, message: "目标目录已有数据" })
      .mockResolvedValueOnce({
        needsConfirmation: false,
        message: "目录已切换，请选择是否清理旧文件并重启",
      });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);

    renderSettings();
    const input = (await screen.findAllByRole("textbox"))[0] as HTMLInputElement;
    fireEvent.change(input, { target: { value: "D:/Mail" } });
    fireEvent.click(screen.getByRole("button", { name: "更改目录" }));

    await waitFor(() => expect(confirm).toHaveBeenCalled());
    await waitFor(() =>
      expect(api.changeDataDir).toHaveBeenNthCalledWith(1, "D:/Mail", false),
    );
    await waitFor(() =>
      expect(api.changeDataDir).toHaveBeenNthCalledWith(2, "D:/Mail", true),
    );
    expect(await screen.findByText("数据目录已经切换")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /清理并重启/ }));
    await waitFor(() => expect(api.restartApp).toHaveBeenCalledWith(true));
  });

  it("迁移失败时显示可读错误，不假装成功", async () => {
    vi.mocked(api.changeDataDir).mockRejectedValue(new Error("目标盘空间不够"));
    renderSettings();

    const input = (await screen.findAllByRole("textbox"))[0] as HTMLInputElement;
    fireEvent.change(input, { target: { value: "D:/Mail" } });
    fireEvent.click(screen.getByRole("button", { name: "更改目录" }));

    expect(await screen.findByText(/目录切换失败：目标盘空间不够/)).toBeTruthy();
    expect(screen.queryByText("数据目录已经切换")).toBeNull();
  });

  it("打开目录只调用后端当前生效目录命令", async () => {
    renderSettings();

    fireEvent.click(await screen.findByRole("button", { name: "打开目录" }));

    await waitFor(() => expect(api.openDataDir).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("已打开当前数据目录。")).toBeTruthy();
  });
});

describe("设置页通讯录分组", () => {
  afterEach(() => {
    cleanup();
  });

  it("清空自动收集要先确认，确认后把删掉的条数说出来", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    vi.mocked(api.clearAutoContacts).mockResolvedValue(7);
    renderSettings();
    await screen.findByText("通讯录");

    fireEvent.click(screen.getByRole("button", { name: "清空自动收集的联系人" }));

    await waitFor(() => expect(api.clearAutoContacts).toHaveBeenCalledTimes(1));
    expect(confirm).toHaveBeenCalled();
    expect(await screen.findByText("已清掉 7 条自动收集的联系人")).toBeTruthy();
    confirm.mockRestore();
  });

  it("点取消就什么都不做", async () => {
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(false);
    renderSettings();
    await screen.findByText("通讯录");

    fireEvent.click(screen.getByRole("button", { name: "清空自动收集的联系人" }));

    expect(api.clearAutoContacts).not.toHaveBeenCalled();
    confirm.mockRestore();
  });
});