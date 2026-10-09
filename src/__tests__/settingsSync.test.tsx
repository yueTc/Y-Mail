//! GitHub 登录与设置同步面板前端回归：
//! - 登录面板：显示用户码 → 轮询成功 → 显示头像；头像只放行固定域名。
//! - 设置同步面板：未勾选、密码不一致、开启成功、冲突选择、权限不够五条路径。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import GitHubLoginPanel, { safeAvatarUrl } from "../GitHubLoginPanel";
import SettingsSyncPanel from "../SettingsSyncPanel";
import type { GitHubLoginView, SettingsSyncStatus } from "../api";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    githubLoginProfile: vi.fn(),
    githubLoginStart: vi.fn(),
    githubLoginPoll: vi.fn(),
    githubLoginSignOut: vi.fn(),
    openExternalUrl: vi.fn(),
    settingsSyncStatus: vi.fn(),
    settingsSyncEnable: vi.fn(),
    settingsSyncDisable: vi.fn(),
    settingsSyncNow: vi.fn(),
    settingsSyncResetPassword: vi.fn(),
    settingsSyncJoin: vi.fn(),
    settingsSyncResolveConflict: vi.fn(),
  },
}));

const loggedInSync: GitHubLoginView = {
  login: "octocat",
  name: "八爪猫",
  avatarUrl: "https://avatars.githubusercontent.com/u/1",
  scope: "sync",
};

const baseStatus: SettingsSyncStatus = {
  enabled: false,
  deviceId: null,
  deviceLabel: null,
  gistId: null,
  gistUrl: null,
  revision: 0,
  remoteRevision: 0,
  lastSyncAt: null,
  lastError: null,
  conflict: null,
};

/** 等某个选择器出现，返回元素本身；等不到就让测试失败。 */
async function findEl(container: HTMLElement, selector: string): Promise<HTMLElement> {
  return waitFor(() => {
    const found = container.querySelector(selector);
    expect(found).not.toBeNull();
    return found as HTMLElement;
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  (api.githubLoginProfile as ReturnType<typeof vi.fn>).mockResolvedValue(loggedInSync);
  (api.settingsSyncStatus as ReturnType<typeof vi.fn>).mockResolvedValue(baseStatus);
  (api.githubLoginSignOut as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  (api.openExternalUrl as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  (api.settingsSyncEnable as ReturnType<typeof vi.fn>).mockImplementation(
    async () => ({ ...baseStatus, enabled: true }) as SettingsSyncStatus,
  );
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("GitHub 登录面板", () => {
  it("显示用户码，轮询成功后显示头像", async () => {
    (api.githubLoginProfile as ReturnType<typeof vi.fn>).mockResolvedValue(null);
    (api.githubLoginStart as ReturnType<typeof vi.fn>).mockResolvedValue({
      loginId: "abc123",
      userCode: "WXYZ-1234",
      verificationUri: "https://github.com/login/device",
      expiresIn: 900,
      interval: 1,
    });
    (api.githubLoginPoll as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce({ status: "pending" })
      .mockResolvedValue({ status: "authorized", ...loggedInSync });

    render(<GitHubLoginPanel />);

    fireEvent.click(await screen.findByRole("button", { name: "用 GitHub 登录" }));

    // 第一轮轮询还在等，界面该显示用户码。
    expect(await screen.findByText("WXYZ-1234")).toBeTruthy();
    expect(api.openExternalUrl).toHaveBeenCalledWith("https://github.com/login/device");

    // 第二轮（1 秒后）返回成功后应显示头像和昵称。
    const avatar = await screen.findByAltText("GitHub 头像", undefined, { timeout: 4000 });
    expect(avatar.getAttribute("src")).toBe("https://avatars.githubusercontent.com/u/1");
    expect(screen.getByText("@octocat")).toBeTruthy();
    expect(screen.queryByText("WXYZ-1234")).toBeNull();
  });

  it("头像只放行固定域名", () => {
    expect(safeAvatarUrl("https://avatars.githubusercontent.com/u/1")).toBe(
      "https://avatars.githubusercontent.com/u/1",
    );
    expect(safeAvatarUrl("http://avatars.githubusercontent.com/u/1")).toBeUndefined();
    expect(safeAvatarUrl("https://evil.example.com/u/1")).toBeUndefined();
    expect(safeAvatarUrl("https://avatars.githubusercontent.com.evil.com/u/1")).toBeUndefined();
    expect(safeAvatarUrl(null)).toBeUndefined();
  });
});

describe("设置同步面板", () => {
  it("没勾选两条说明时开不了并提示", async () => {
    const { container } = render(<SettingsSyncPanel />);
    const form = await findEl(container, ".settings-sync-enable");

    fireEvent.change(within(form).getByLabelText("同步密码"), {
      target: { value: "password123" },
    });
    fireEvent.change(within(form).getByLabelText("再输一次"), {
      target: { value: "password123" },
    });
    fireEvent.change(within(form).getByLabelText("这台设备的名字"), {
      target: { value: "测试机" },
    });

    fireEvent.click(within(form).getByRole("button", { name: "开启同步" }));

    expect(await screen.findByText("请先勾选上面两条说明。")).toBeTruthy();
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
  });

  it("两次密码不一致时开不了并提示", async () => {
    const { container } = render(<SettingsSyncPanel />);
    const form = await findEl(container, ".settings-sync-enable");

    fireEvent.click(within(form).getByLabelText(/私密 Gist/));
    fireEvent.click(within(form).getByLabelText(/敏感信息/));
    fireEvent.change(within(form).getByLabelText("同步密码"), {
      target: { value: "password123" },
    });
    fireEvent.change(within(form).getByLabelText("再输一次"), {
      target: { value: "password456" },
    });
    fireEvent.change(within(form).getByLabelText("这台设备的名字"), {
      target: { value: "测试机" },
    });

    fireEvent.click(within(form).getByRole("button", { name: "开启同步" }));

    expect(await screen.findByText("两次输入的同步密码不一致。")).toBeTruthy();
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
  });

  it("勾选、密码一致、有设备名时能开启同步", async () => {
    const { container } = render(<SettingsSyncPanel />);
    const form = await findEl(container, ".settings-sync-enable");

    fireEvent.click(within(form).getByLabelText(/私密 Gist/));
    fireEvent.click(within(form).getByLabelText(/敏感信息/));
    fireEvent.change(within(form).getByLabelText("同步密码"), {
      target: { value: "password123" },
    });
    fireEvent.change(within(form).getByLabelText("再输一次"), {
      target: { value: "password123" },
    });
    fireEvent.change(within(form).getByLabelText("这台设备的名字"), {
      target: { value: "测试机" },
    });

    fireEvent.click(within(form).getByRole("button", { name: "开启同步" }));

    await waitFor(() =>
      expect(api.settingsSyncEnable).toHaveBeenCalledWith("password123", "password123", "测试机"),
    );
  });

  it("出现冲突时能让用户选保留哪边", async () => {
    (api.settingsSyncStatus as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...baseStatus,
      enabled: true,
      revision: 3,
      remoteRevision: 4,
      conflict: {
        remoteDeviceId: "dev-2",
        remoteDeviceLabel: "公司的笔记本",
        remoteRevision: 4,
        updatedAt: "2026-10-09T00:00:00Z",
      },
    });
    (api.settingsSyncResolveConflict as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...baseStatus,
      enabled: true,
    });

    const { container } = render(<SettingsSyncPanel />);
    const conflict = await findEl(container, ".settings-sync-conflict");

    expect(within(conflict).getByText(/公司的笔记本/)).toBeTruthy();

    fireEvent.click(within(conflict).getByRole("button", { name: "采用远端配置" }));

    await waitFor(() => expect(api.settingsSyncResolveConflict).toHaveBeenCalledWith("keepRemote"));
  });

  it("登录权限不够时提示先授权 Gist 权限", async () => {
    (api.githubLoginProfile as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...loggedInSync,
      scope: "login",
    });

    const { container } = render(<SettingsSyncPanel />);

    expect(
      await screen.findByText("设置同步需要「管理你的 Gist」权限，请在上面再授权一次。"),
    ).toBeTruthy();
    await waitFor(() => expect(container.querySelector(".settings-sync-enable")).toBeNull());
  });
});