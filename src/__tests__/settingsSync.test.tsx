//! GitHub 登录与设置同步面板前端回归：
//! - 登录面板：显示用户码 → 轮询成功 → 显示头像；头像优先用本机缓存，只放行固定域名。

//! - 设置同步面板：单次密码上传、云端覆盖确认、共用密码下载、本机覆盖确认与取消。

import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import GitHubLoginPanel, { safeAvatarDataUrl, safeAvatarUrl } from "../GitHubLoginPanel";
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
    getComputerName: vi.fn(),
    settingsSyncEnable: vi.fn(),
    settingsSyncDisable: vi.fn(),
    settingsSyncNow: vi.fn(),
    settingsSyncResetPassword: vi.fn(),
    settingsSyncDownload: vi.fn(),
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
  vi.resetAllMocks();
  (api.githubLoginProfile as ReturnType<typeof vi.fn>).mockResolvedValue(loggedInSync);
  (api.settingsSyncStatus as ReturnType<typeof vi.fn>).mockResolvedValue(baseStatus);
  (api.getComputerName as ReturnType<typeof vi.fn>).mockResolvedValue("测试电脑");
  (api.settingsSyncDownload as ReturnType<typeof vi.fn>).mockResolvedValue({ ...baseStatus, enabled: true });
  (api.githubLoginSignOut as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  (api.openExternalUrl as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
  (api.settingsSyncEnable as ReturnType<typeof vi.fn>).mockImplementation(
    async () => ({ ...baseStatus, enabled: true }) as SettingsSyncStatus,
  );
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.resetAllMocks();
});

describe("设备名默认值", () => {
  it("默认填入电脑名称，用户修改后保留修改值", async () => {
    render(<SettingsSyncPanel />);

    const form = await findEl(document.body, ".settings-sync-enable");
    const input = within(form).getByLabelText("这台设备的名字") as HTMLInputElement;
    await waitFor(() => expect(input.value).toBe("测试电脑"));

    fireEvent.change(input, { target: { value: "手动设备名" } });
    await waitFor(() => expect(input.value).toBe("手动设备名"));
  });
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

  it("有本机缓存时优先用缓存头像", async () => {
    const cached =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg==";
    (api.githubLoginProfile as ReturnType<typeof vi.fn>).mockResolvedValue({
      ...loggedInSync,
      avatarDataUrl: cached,
    });

    render(<GitHubLoginPanel />);

    const avatar = await screen.findByAltText("GitHub 头像");
    expect(avatar.getAttribute("src")).toBe(cached);
  });

  it("本机缓存头像只放行图片 data URL", () => {
    const png = "data:image/png;base64,iVBORw0KGgo=";
    expect(safeAvatarDataUrl(png)).toBe(png);
    expect(safeAvatarDataUrl("data:text/html;base64,PHNjcmlwdD4=")).toBeUndefined();
    expect(safeAvatarDataUrl("javascript:alert(1)")).toBeUndefined();
    expect(safeAvatarDataUrl("https://avatars.githubusercontent.com/u/1")).toBeUndefined();
    expect(safeAvatarDataUrl(null)).toBeUndefined();
  });

  it("退出登录后头像消失", async () => {
    render(<GitHubLoginPanel />);
    expect(await screen.findByAltText("GitHub 头像")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "退出登录" }));

    await waitFor(() => expect(screen.queryByAltText("GitHub 头像")).toBeNull());
    expect(api.githubLoginSignOut).toHaveBeenCalled();
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
  async function fillForm({
    agree = true,
    password = "password123",
    onSettingsImported,
  }: {
    agree?: boolean;
    password?: string;
    onSettingsImported?: () => void;
  } = {}) {
    const { container } = render(
      <SettingsSyncPanel onSettingsImported={onSettingsImported} />,
    );
    const form = await findEl(container, ".settings-sync-enable");
    if (agree) {
      fireEvent.click(within(form).getByLabelText(/私密 Gist/));
      fireEvent.click(within(form).getByLabelText(/敏感信息/));
    }
    fireEvent.change(within(form).getByLabelText("同步密码"), {
      target: { value: password },
    });
    fireEvent.change(within(form).getByLabelText("这台设备的名字"), {
      target: { value: "测试机" },
    });
    return form;
  }

  it("没勾选两条说明时不能上传并提示", async () => {
    const form = await fillForm({ agree: false });
    fireEvent.click(within(form).getByRole("button", { name: "上传到云端" }));
    expect(await screen.findByText("请先勾选上面两条说明。")).toBeTruthy();
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
  });

  it("同步密码不足 8 位时不能上传并提示", async () => {
    const form = await fillForm({ password: "short" });
    fireEvent.click(within(form).getByRole("button", { name: "上传到云端" }));
    expect(await screen.findByText("同步密码至少 8 位。")).toBeTruthy();
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
  });

  it("只输入一次密码即可上传，删除重复密码和第二台设备入口", async () => {
    const form = await fillForm();
    const confirm = vi.spyOn(window, "confirm");
    expect(form.querySelectorAll('input[type="password"]')).toHaveLength(1);
    expect(screen.queryByText("再输一次")).toBeNull();
    expect(screen.queryByText(/这是第二台设备/)).toBeNull();
    const upload = within(form).getByRole("button", { name: "上传到云端" });
    const download = within(form).getByRole("button", { name: "从云端下载" });
    expect(upload.nextElementSibling).toBe(download);
    fireEvent.click(upload);
    await waitFor(() =>
      expect(api.settingsSyncEnable).toHaveBeenCalledWith("password123", "测试机", false),
    );
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect((screen.getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
    expect(confirm).not.toHaveBeenCalled();
  });

  it.each([true, false])("云端已有内容时确认覆盖为 %s 才决定是否重传", async (overwrite) => {
    vi.mocked(api.settingsSyncEnable).mockRejectedValueOnce(new Error("云端已有同步内容，请确认覆盖后重试"));
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(overwrite);
    const form = await fillForm();
    fireEvent.click(within(form).getByRole("button", { name: "上传到云端" }));
    await waitFor(() => expect(confirm).toHaveBeenCalledWith(expect.stringContaining("云端已有同步内容")));
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect(api.settingsSyncEnable).toHaveBeenCalledTimes(overwrite ? 2 : 1);
    expect(api.settingsSyncEnable).toHaveBeenNthCalledWith(1, "password123", "测试机", false);
    if (overwrite) {
      expect(api.settingsSyncEnable).toHaveBeenNthCalledWith(2, "password123", "测试机", true);
    } else {
      expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
      expect(screen.queryByRole("alert")).toBeNull();
    }
  });

  it("上传普通错误直接显示，确认覆盖后的失败也能显示并重试", async () => {
    vi.mocked(api.settingsSyncEnable)
      .mockRejectedValueOnce(new Error("网络不可用"))
      .mockRejectedValueOnce(new Error("云端已有同步内容，请确认覆盖后重试"))
      .mockRejectedValueOnce(new Error("覆盖失败"));
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(true);
    const form = await fillForm();
    const upload = within(form).getByRole("button", { name: "上传到云端" });
    fireEvent.click(upload);
    expect(await screen.findByText("网络不可用")).toBeTruthy();
    expect(confirm).not.toHaveBeenCalled();
    fireEvent.click(upload);
    expect(await screen.findByText("覆盖失败")).toBeTruthy();
    expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
    expect((upload as HTMLButtonElement).disabled).toBe(false);
  });

  it("下载直接复用上面的同步密码和设备名", async () => {
    const form = await fillForm({ agree: false });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    await waitFor(() =>
      expect(api.settingsSyncDownload).toHaveBeenCalledWith("password123", "测试机", false),
    );
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect((screen.getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
  });

  it("没有输入同步密码时不能下载", async () => {
    const form = await fillForm({ password: "" });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    expect(await screen.findByText("请填写同步密码。")).toBeTruthy();
    expect(api.settingsSyncDownload).not.toHaveBeenCalled();
  });

  it.each(["上传到云端", "从云端下载"])("%s 前要求设备名非空", async (action) => {
    const form = await fillForm();
    fireEvent.change(within(form).getByLabelText("这台设备的名字"), { target: { value: "  " } });
    fireEvent.click(within(form).getByRole("button", { name: action }));
    expect(await screen.findByText("请填写这台设备的名字。")).toBeTruthy();
    expect(api.settingsSyncEnable).not.toHaveBeenCalled();
    expect(api.settingsSyncDownload).not.toHaveBeenCalled();
  });

  it.each([true, false])("下载覆盖本机配置的确认结果为 %s", async (overwrite) => {
    vi.mocked(api.settingsSyncDownload).mockRejectedValueOnce(
      new Error("云端配置会覆盖本机这些条目：邮箱 a@example.com。请确认是否覆盖"),
    );
    const confirm = vi.spyOn(window, "confirm").mockReturnValue(overwrite);
    const form = await fillForm({ agree: false });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    await waitFor(() => expect(confirm).toHaveBeenCalledWith(expect.stringContaining("a@example.com")));
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect(api.settingsSyncDownload).toHaveBeenCalledTimes(overwrite ? 2 : 1);
    expect(api.settingsSyncDownload).toHaveBeenNthCalledWith(1, "password123", "测试机", false);
    if (overwrite) {
      expect(api.settingsSyncDownload).toHaveBeenNthCalledWith(2, "password123", "测试机", true);
    } else {
      expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
      expect(screen.queryByRole("alert")).toBeNull();
    }
  });

  it("下载密码错误时保留输入并显示错误，不弹覆盖确认", async () => {
    vi.mocked(api.settingsSyncDownload).mockRejectedValueOnce(new Error("密码错误或同步内容已损坏"));
    const confirm = vi.spyOn(window, "confirm");
    const form = await fillForm({ agree: false });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    expect(await screen.findByText("密码错误或同步内容已损坏")).toBeTruthy();
    expect(confirm).not.toHaveBeenCalled();
    expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
  });

  it("传输期间两个操作按钮都禁用，成功后保留密码", async () => {
    let finish!: (status: SettingsSyncStatus) => void;
    vi.mocked(api.settingsSyncDownload).mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    const form = await fillForm({ agree: false });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    expect((within(form).getByRole("button", { name: "上传到云端" }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(form).getByRole("button", { name: "从云端下载" }) as HTMLButtonElement).disabled).toBe(true);
    finish({ ...baseStatus, enabled: true });
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect((screen.getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
  });

  it("已有同步记录和冲突时仍只显示原来的上传下载表单", async () => {
    vi.mocked(api.settingsSyncStatus).mockResolvedValue({
      ...baseStatus,
      enabled: true,
      deviceLabel: "保存的设备名",
      gistUrl: "https://gist.github.com/saved",
      revision: 3,
      remoteRevision: 4,
      conflict: {
        remoteDeviceId: "dev-2",
        remoteDeviceLabel: "公司的笔记本",
        remoteRevision: 4,
        updatedAt: "2026-10-09T00:00:00Z",
      },
    });
    const { container } = render(<SettingsSyncPanel />);
    const form = await findEl(container, ".settings-sync-enable");
    await waitFor(() =>
      expect((within(form).getByLabelText("这台设备的名字") as HTMLInputElement).value).toBe("保存的设备名"),
    );
    expect(within(form).getByRole("button", { name: "上传到云端" })).toBeTruthy();
    expect(within(form).getByRole("button", { name: "从云端下载" })).toBeTruthy();
    expect(screen.queryByText("重设同步密码")).toBeNull();
    expect(screen.queryByText("当前版本：")).toBeNull();
    expect(screen.queryByRole("button", { name: "立即同步" })).toBeNull();
    expect(screen.queryByRole("button", { name: "关闭同步" })).toBeNull();
    expect(screen.queryByRole("button", { name: "在浏览器里打开 Gist" })).toBeNull();
    expect(container.querySelector(".settings-sync-conflict")).toBeNull();
    expect(container.querySelector("details")).toBeNull();
  });

  it.each(["上传到云端", "从云端下载"])("%s 成功后保留同一表单并且可以再次操作", async (action) => {
    const form = await fillForm();
    const operation = action === "上传到云端" ? api.settingsSyncEnable : api.settingsSyncDownload;
    const result = action === "上传到云端" ? "已上传到云端。" : "已从云端下载。";
    fireEvent.click(within(form).getByRole("button", { name: action }));
    expect(await screen.findByText(result)).toBeTruthy();
    expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("password123");
    expect(screen.queryByText("重设同步密码")).toBeNull();
    expect(screen.queryByRole("button", { name: "关闭同步" })).toBeNull();
    expect(within(form).getAllByRole("button")).toHaveLength(2);
    fireEvent.change(within(form).getByLabelText("同步密码"), { target: { value: "second-password" } });
    fireEvent.click(within(form).getByRole("button", { name: action }));
    await waitFor(() => expect(operation).toHaveBeenCalledTimes(2));
    expect(operation).toHaveBeenNthCalledWith(2, "second-password", "测试机", false);
    await waitFor(() => expect(document.querySelector('[aria-busy="true"]')).toBeNull());
    expect((within(form).getByLabelText("同步密码") as HTMLInputElement).value).toBe("second-password");
  });

  it("下载成功后通知外层重读账号", async () => {
    const onSettingsImported = vi.fn();
    const form = await fillForm({ onSettingsImported });
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    expect(await screen.findByText("已从云端下载。")).toBeTruthy();
    expect(onSettingsImported).toHaveBeenCalledTimes(1);
  });

  it("上传成功后直接复用保留的密码下载", async () => {
    const form = await fillForm();
    fireEvent.click(within(form).getByRole("button", { name: "上传到云端" }));
    expect(await screen.findByText("已上传到云端。")).toBeTruthy();
    fireEvent.click(within(form).getByRole("button", { name: "从云端下载" }));
    expect(await screen.findByText("已从云端下载。")).toBeTruthy();
    expect(api.settingsSyncDownload).toHaveBeenCalledWith("password123", "测试机", false);
    expect(screen.queryByText("已上传到云端。")).toBeNull();
    expect(within(form).getAllByRole("button")).toHaveLength(2);
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