//! 设置同步导入账号后，设置里的账号列表要立即重读。

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AccountPanel from "../AccountPanel";
import { api, type Account } from "../api";

vi.mock("../AccountDialog", () => ({ default: () => null }));

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listAccounts: vi.fn(),
    testSavedAccount: vi.fn(),
    deleteAccount: vi.fn(),
  },
}));

function account(id: number, displayName: string): Account {
  return {
    id,
    displayName,
    email: `mail${id}@example.com`,
    authType: "password",
    username: `mail${id}@example.com`,
    imap: { host: "imap.example.com", port: 993, security: "tls" },
    smtp: { host: "smtp.example.com", port: 465, security: "tls" },
    proxy: { mode: "inherit" },
    color: "#3366ff",
    enabled: true,
    oauthProvider: null,
    oauthClientId: "",
    hasCredential: true,
    createdAt: "2026-10-21T00:00:00Z",
    updatedAt: "2026-10-21T00:00:00Z",
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.listAccounts).mockResolvedValue([account(1, "一号邮箱")]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("账号列表刷新", () => {
  it("账号版本变化后重新读取并显示新导入的账号", async () => {
    const view = render(<AccountPanel proxiesVersion={0} accountsVersion={0} />);
    expect(await screen.findByText("一号邮箱")).toBeTruthy();
    expect(api.listAccounts).toHaveBeenCalledTimes(1);

    vi.mocked(api.listAccounts).mockResolvedValue([
      account(1, "一号邮箱"),
      account(2, "二号邮箱"),
    ]);
    view.rerender(<AccountPanel proxiesVersion={0} accountsVersion={1} />);

    expect(await screen.findByText("二号邮箱")).toBeTruthy();
    expect(api.listAccounts).toHaveBeenCalledTimes(2);
  });
});