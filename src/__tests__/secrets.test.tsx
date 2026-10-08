//! 前端安全回归：授权码与代理密码不回显、不落浏览器本地存储。
//!
//! 渲染真实的账号 / 代理面板，只把 Tauri 命令层（./api）换成内存桩，
//! 这样断言的正是用户实际看到的输入框行为。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AccountPanel from "../AccountPanel";
import ProxyPanel from "../ProxyPanel";
import { api, type Account, type Proxy } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listAccounts: vi.fn(),
    testAccountConnection: vi.fn(),
    createAccount: vi.fn(),
    updateAccount: vi.fn(),
    deleteAccount: vi.fn(),
    testSavedAccount: vi.fn(),
    listProxies: vi.fn(),
    saveProxy: vi.fn(),
    deleteProxy: vi.fn(),
    getProxySettings: vi.fn(),
    setProxySettings: vi.fn(),
    testProxy: vi.fn(),
    beginOAuthAuthorize: vi.fn(),
    completeOAuthAuthorize: vi.fn(),
    cancelOAuthAuthorize: vi.fn(),
    oauthStatus: vi.fn(),
  },
}));

const SAVED_ACCOUNT: Account = {
  id: 7,
  displayName: "我的邮箱",
  email: "someone@example.com",
  authType: "password",
  username: "someone@example.com",
  imap: { host: "imap.example.com", port: 993, security: "tls" },
  smtp: { host: "smtp.example.com", port: 465, security: "tls" },
  proxy: { mode: "inherit" },
  color: "#3366ff",
  enabled: true,
  oauthProvider: null,
  oauthClientId: "",
  hasCredential: true,
  createdAt: "2026-10-04T00:00:00Z",
  updatedAt: "2026-10-04T00:00:00Z",
};

const SAVED_PROXY: Proxy = {
  id: 3,
  label: "公司代理",
  kind: "socks5",
  host: "127.0.0.1",
  port: 1080,
  username: "proxy-user",
  hasPassword: true,
};

function secretInput(label: RegExp): HTMLInputElement {
  const input = screen.getByLabelText(label);
  expect(input).toBeInstanceOf(HTMLInputElement);
  return input as HTMLInputElement;
}

beforeEach(() => {
  vi.mocked(api.listAccounts).mockResolvedValue([]);
  vi.mocked(api.listProxies).mockResolvedValue([]);
  vi.mocked(api.getProxySettings).mockResolvedValue({ mode: "system" });
  window.localStorage.clear();
  window.sessionStorage.clear();
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  window.localStorage.clear();
  window.sessionStorage.clear();
});

describe("账号面板的授权码输入", () => {
  it("新建时用密码框且初始为空", async () => {
    render(<AccountPanel proxiesVersion={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增账号" }));

    const input = secretInput(/授权码 \/ 密码/);
    expect(input.type).toBe("password");
    expect(input.value).toBe("");
    expect(input.getAttribute("autocomplete")).toBe("off");
  });

  it("显示名留空时保存用邮箱地址兜底", async () => {
    vi.mocked(api.testAccountConnection).mockResolvedValue({
      imapFolderCount: 3,
      smtpMechanism: "PLAIN",
    });
    vi.mocked(api.createAccount).mockResolvedValue(SAVED_ACCOUNT);

    render(<AccountPanel proxiesVersion={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增账号" }));

    const displayName = screen.getByLabelText(/显示名/) as HTMLInputElement;
    expect(displayName.placeholder).toBe("可留空，默认用邮箱地址");
    expect(displayName.value).toBe("");

    fireEvent.change(screen.getByLabelText(/邮箱地址/), {
      target: { value: "someone@example.com" },
    });
    fireEvent.change(screen.getByLabelText(/登录名/), {
      target: { value: "someone@example.com" },
    });
    fireEvent.change(secretInput(/授权码 \/ 密码/), { target: { value: "auth-code" } });

    fireEvent.click(screen.getByRole("button", { name: "连接自检" }));
    await screen.findByText(/自检通过/);
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await vi.waitFor(() => expect(api.createAccount).toHaveBeenCalledTimes(1));
    const [sentDraft, sentSecret] = vi.mocked(api.createAccount).mock.calls[0];
    expect(sentDraft.displayName).toBe("someone@example.com");
    expect(sentSecret).toBe("auth-code");
  });
  it("编辑已保存账号时不会把旧授权码回显出来", async () => {
    vi.mocked(api.listAccounts).mockResolvedValue([SAVED_ACCOUNT]);
    render(<AccountPanel proxiesVersion={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑" }));

    const input = secretInput(/新的授权码/);
    expect(input.type).toBe("password");
    expect(input.value).toBe("");
  });
});

describe("代理面板的密码输入", () => {
  it("新建时用密码框且初始为空", async () => {
    render(<ProxyPanel onChanged={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增代理" }));

    const input = secretInput(/代理密码/);
    expect(input.type).toBe("password");
    expect(input.value).toBe("");
    expect(input.getAttribute("autocomplete")).toBe("off");
  });

  it("编辑已保存代理时不会把旧密码回显出来", async () => {
    vi.mocked(api.listProxies).mockResolvedValue([SAVED_PROXY]);
    render(<ProxyPanel onChanged={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "编辑" }));

    const input = secretInput(/新的代理密码/);
    expect(input.type).toBe("password");
    expect(input.value).toBe("");
  });
});

describe("敏感值不落浏览器本地存储", () => {
  it("输入授权码与代理密码后，本地存储里搜不到明文", async () => {
    const accountSecret = "auth-code-SECRET-123";
    const proxySecret = "proxy-pass-SECRET-456";
    const setItem = vi.spyOn(Storage.prototype, "setItem");

    const account = render(<AccountPanel proxiesVersion={0} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增账号" }));
    const accountInput = secretInput(/授权码 \/ 密码/);
    fireEvent.change(accountInput, { target: { value: accountSecret } });
    expect(accountInput.value).toBe(accountSecret);
    account.unmount();

    render(<ProxyPanel onChanged={() => {}} />);
    fireEvent.click(await screen.findByRole("button", { name: "新增代理" }));
    const proxyInput = secretInput(/代理密码/);
    fireEvent.change(proxyInput, { target: { value: proxySecret } });
    expect(proxyInput.value).toBe(proxySecret);

    // 本地存储里既没有直接写入，也没有这些明文。
    for (const [, value] of setItem.mock.calls) {
      expect(String(value)).not.toContain(accountSecret);
      expect(String(value)).not.toContain(proxySecret);
    }
    for (const storage of [window.localStorage, window.sessionStorage]) {
      for (let index = 0; index < storage.length; index += 1) {
        const key = storage.key(index);
        expect(key === null ? "" : String(storage.getItem(key))).not.toContain(accountSecret);
        expect(key === null ? "" : String(storage.getItem(key))).not.toContain(proxySecret);
      }
    }
  });

  it("盯着敏感字段的那几个源文件里没有浏览器存储写入", () => {
    const sources = import.meta.glob("../**/*.{ts,tsx}", {
      query: "?raw",
      import: "default",
      eager: true,
    }) as Record<string, string>;
    const storageWrite = /\b(?:window\.|globalThis\.)?(?:localStorage|sessionStorage)\s*(?:\.|\[)/;

    for (const [path, text] of Object.entries(sources)) {
      if (path.includes("__tests__") || path.includes(".test.")) continue;
      if (!/(api\.ts|AccountPanel\.tsx|ProxyPanel\.tsx)$/.test(path)) continue;
      expect(storageWrite.test(text), `${path} 不应碰浏览器本地存储`).toBe(false);
    }
  });
});