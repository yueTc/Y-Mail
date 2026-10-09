//! 「关于」页回归：版本号、打开项目页面、查更新、下载安装并重启。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AboutPanel from "../AboutPanel";
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
    appVersion: vi.fn(),
    checkForUpdate: vi.fn(),
    installPendingUpdate: vi.fn(),
    relaunchApp: vi.fn(),
    openExternalUrl: vi.fn(),
  },
}));

// 设置页复用了其它面板；这里只测「关于」，把它们替换成空壳。
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

const UPDATE_INFO = {
  version: "0.1.3",
  currentVersion: "0.1.2",
  notes: "修了几个问题",
  date: "2026-10-08T00:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(api.appVersion).mockResolvedValue("0.1.2");
  vi.mocked(api.getAppSettings).mockResolvedValue(SETTINGS);
  vi.mocked(api.autostartStatus).mockResolvedValue(false);
  vi.mocked(api.checkForUpdate).mockResolvedValue(null);
  vi.mocked(api.installPendingUpdate).mockResolvedValue(undefined);
  vi.mocked(api.relaunchApp).mockResolvedValue(undefined);
  vi.mocked(api.openExternalUrl).mockResolvedValue(undefined);
  vi.mocked(api.setUpdateSettings).mockImplementation(async (autoCheckUpdate, hours) => ({
    ...SETTINGS,
    autoCheckUpdate,
    updateCheckIntervalHours: hours,
  }));
});

afterEach(cleanup);

describe("关于页", () => {
  it("显示后端读到的当前版本", async () => {
    render(<AboutPanel />);
    expect(await screen.findByText("Y-Mail 0.1.2")).toBeTruthy();
  });

  it("查到新版本时显示版本号、发布说明和安装按钮", async () => {
    vi.mocked(api.checkForUpdate).mockResolvedValue(UPDATE_INFO);
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("发现新版本 0.1.3（当前 0.1.2）。")).toBeTruthy();
    expect(screen.getByText("修了几个问题")).toBeTruthy();
    expect(screen.getByRole("button", { name: "下载并安装" })).toBeTruthy();
  });

  it("没有新版本时提示已是最新", async () => {
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("已经是最新版本。")).toBeTruthy();
  });

  it("查更新失败时把原因显示出来", async () => {
    vi.mocked(api.checkForUpdate).mockRejectedValue(new Error("连不上更新服务器"));
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("检查更新失败：连不上更新服务器")).toBeTruthy();
  });

  it("下载安装后调用重启", async () => {
    vi.mocked(api.checkForUpdate).mockResolvedValue(UPDATE_INFO);
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    fireEvent.click(await screen.findByRole("button", { name: "下载并安装" }));

    await waitFor(() => {
      expect(api.installPendingUpdate).toHaveBeenCalledTimes(1);
      expect(api.relaunchApp).toHaveBeenCalledTimes(1);
    });
  });

  it("安装失败时提示原因且不重启", async () => {
    vi.mocked(api.checkForUpdate).mockResolvedValue(UPDATE_INFO);
    vi.mocked(api.installPendingUpdate).mockRejectedValue(new Error("签名对不上"));
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "检查更新" }));
    fireEvent.click(await screen.findByRole("button", { name: "下载并安装" }));

    expect(await screen.findByText("安装更新失败：签名对不上")).toBeTruthy();
    expect(api.relaunchApp).not.toHaveBeenCalled();
  });

  it("打开 GitHub 页面走系统浏览器，地址写死是仓库地址", async () => {
    render(<AboutPanel />);

    fireEvent.click(screen.getByRole("button", { name: "打开 GitHub 页面" }));

    await waitFor(() => {
      expect(api.openExternalUrl).toHaveBeenCalledWith("https://github.com/yueTc/Y-Mail");
    });
  });

  it("设置页左侧有「关于」分类，点开能看到版本", async () => {
    render(<SettingsWorkspace proxiesVersion={1} onProxiesChanged={() => {}} />);

    const navItem = screen.getByRole("button", { name: "关于" });
    fireEvent.click(navItem);

    expect(navItem.getAttribute("aria-current")).toBe("true");
    expect(await screen.findByText("Y-Mail 0.1.2")).toBeTruthy();
  });

  it("自动检测更新默认关闭，间隔显示 24", async () => {
    render(<AboutPanel />);

    const toggle = (await screen.findByRole("checkbox", {
      name: /自动检测更新/,
    })) as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    const interval = screen.getByRole("spinbutton") as HTMLInputElement;
    expect(interval.value).toBe("24");
  });

  it("打开自动检测开关会按当前间隔保存", async () => {
    render(<AboutPanel />);

    const toggle = await screen.findByRole("checkbox", { name: /自动检测更新/ });
    fireEvent.click(toggle);

    await waitFor(() => expect(api.setUpdateSettings).toHaveBeenCalledWith(true, 24));
  });

  it("间隔填合法值立即保存", async () => {
    render(<AboutPanel />);

    const interval = (await screen.findByRole("spinbutton")) as HTMLInputElement;
    fireEvent.change(interval, { target: { value: "6" } });

    await waitFor(() => expect(api.setUpdateSettings).toHaveBeenCalledWith(false, 6));
  });

  it("间隔超出范围给中文提示且不保存", async () => {
    render(<AboutPanel />);

    const interval = (await screen.findByRole("spinbutton")) as HTMLInputElement;
    fireEvent.change(interval, { target: { value: "200" } });

    expect(await screen.findByText("检测间隔要填 1 到 168 之间的整数小时。")).toBeTruthy();
    expect(api.setUpdateSettings).not.toHaveBeenCalled();
  });

  it("手动检查发现新版本会上报版本号", async () => {
    vi.mocked(api.checkForUpdate).mockResolvedValue(UPDATE_INFO);
    const onUpdateFound = vi.fn();
    render(<AboutPanel onUpdateFound={onUpdateFound} />);

    fireEvent.click(await screen.findByRole("button", { name: "检查更新" }));

    await waitFor(() => expect(onUpdateFound).toHaveBeenCalledWith("0.1.3"));
  });

  it("有待提醒的新版本时左栏「关于」显示小红点，点开上报已读", async () => {
    const onAboutOpened = vi.fn();
    render(
      <SettingsWorkspace
        proxiesVersion={1}
        onProxiesChanged={() => {}}
        hasUpdateNotice
        onAboutOpened={onAboutOpened}
      />,
    );

    expect(await screen.findByText("有新版本")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /关于/ }));

    expect(onAboutOpened).toHaveBeenCalledTimes(1);
  });
});