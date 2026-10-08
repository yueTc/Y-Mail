//! 首次启动向导回归：用默认位置、选文件夹迁移、失败不假装成功、重启。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import FirstRunWizard from "../FirstRunWizard";
import { api, type AppSettings } from "../api";
import { open } from "@tauri-apps/plugin-dialog";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    saveAppSettings: vi.fn(),
    changeDataDir: vi.fn(),
    restartApp: vi.fn(),
  },
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const SETTINGS: AppSettings = {
  dataDir: "",
  attachmentDir: "",
  notifyNewMail: true,
  notifyAiEnabled: false,
  blockRemoteImagesByDefault: true,
  minimizeToTrayOnClose: true,
  startMinimizedToTray: false,
  defaultDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  defaultAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  activeDataDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop",
  activeAttachmentDir: "C:/Users/me/AppData/Roaming/com.ymail.desktop/downloads",
  firstRun: true,
};

function renderWizard() {
  const onDone = vi.fn();
  render(<FirstRunWizard settings={SETTINGS} onDone={onDone} />);
  return onDone;
}

describe("首次启动向导", () => {
  beforeEach(() => {
    vi.mocked(api.saveAppSettings).mockResolvedValue({ ...SETTINGS, firstRun: false });
    vi.mocked(api.changeDataDir).mockResolvedValue({
      needsConfirmation: false,
      message: "目录已切换，重启后生效",
    });
    vi.mocked(api.restartApp).mockResolvedValue(undefined);
    vi.mocked(open).mockResolvedValue("D:/Mail");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.clearAllMocks();
  });

  it("用默认位置：保存一份默认设置后就收工", async () => {
    const onDone = renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /用默认位置/ }));

    await waitFor(() =>
      expect(api.saveAppSettings).toHaveBeenCalledWith({
        dataDir: "",
        attachmentDir: "",
        notifyNewMail: true,
        notifyAiEnabled: false,
        blockRemoteImagesByDefault: true,
      }),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalled());
    expect(api.changeDataDir).not.toHaveBeenCalled();
  });

  it("选择文件夹：迁移成功后提示重启，点重启调用重启命令", async () => {
    const onDone = renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));

    await waitFor(() => expect(api.changeDataDir).toHaveBeenCalledWith("D:/Mail", false));
    expect(await screen.findByText("数据目录已经切换")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: /清理并重启/ }));
    await waitFor(() => expect(api.restartApp).toHaveBeenCalledWith(true));
    expect(onDone).not.toHaveBeenCalled();
  });

  it("选择直接重启：明确告诉后端不清理旧文件", async () => {
    renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));
    await screen.findByText("数据目录已经切换");

    fireEvent.click(screen.getByRole("button", { name: /直接重启（不清理旧文件）/ }));
    await waitFor(() => expect(api.restartApp).toHaveBeenCalledWith(false));
  });

  it("选的目录就是默认目录：按用默认处理，不跑迁移", async () => {
    vi.mocked(open).mockResolvedValue(SETTINGS.defaultDataDir);
    const onDone = renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));

    await waitFor(() => expect(api.saveAppSettings).toHaveBeenCalled());
    expect(api.changeDataDir).not.toHaveBeenCalled();
    await waitFor(() => expect(onDone).toHaveBeenCalled());
  });

  it("用户在系统对话框取消：什么都不做", async () => {
    vi.mocked(open).mockResolvedValue(null);
    renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));

    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(api.saveAppSettings).not.toHaveBeenCalled();
    expect(api.changeDataDir).not.toHaveBeenCalled();
  });

  it("迁移失败：显示可读错误，不出现重启按钮", async () => {
    vi.mocked(api.changeDataDir).mockRejectedValue(new Error("目标盘空间不够"));
    renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));

    expect(await screen.findByText(/设置数据目录失败：目标盘空间不够/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /清理并重启/ })).toBeNull();
  });

  it("目标已有数据：先确认，确认后带 confirmed 重试", async () => {
    vi.mocked(api.changeDataDir)
      .mockResolvedValueOnce({ needsConfirmation: true, message: "目标目录已有数据" })
      .mockResolvedValueOnce({ needsConfirmation: false, message: "目录已切换，重启后生效" });
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    renderWizard();

    fireEvent.click(screen.getByRole("button", { name: /选择文件夹/ }));

    await waitFor(() => expect(confirm).toHaveBeenCalled());
    await waitFor(() =>
      expect(api.changeDataDir).toHaveBeenNthCalledWith(2, "D:/Mail", true),
    );
    expect(await screen.findByRole("button", { name: /清理并重启/ })).toBeTruthy();
    confirm.mockRestore();
  });
});
