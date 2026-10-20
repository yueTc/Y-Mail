//! 常用邮箱自动填回归：离开邮箱框或按回车自动补齐，手工改过的服务器参数不被覆盖。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AccountForm from "../AccountForm";
import { api, type Account, type OAuthOutcome } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listProxies: vi.fn(),
    oauthStatus: vi.fn(),
    cancelOAuthAuthorize: vi.fn(),
    testAccountConnection: vi.fn(),
    createAccount: vi.fn(),
    updateAccount: vi.fn(),
    beginOAuthAuthorize: vi.fn(),
    completeOAuthAuthorize: vi.fn(),
  },
}));

function input(label: string): HTMLInputElement {
  return screen.getByLabelText(label) as HTMLInputElement;
}

function select(label: string): HTMLSelectElement {
  return screen.getByLabelText(label) as HTMLSelectElement;
}

function renderForm() {
  return render(<AccountForm account={null} onSaved={() => {}} onCancel={() => {}} />);
}

beforeEach(() => {
  vi.mocked(api.listProxies).mockResolvedValue([]);
  vi.mocked(api.cancelOAuthAuthorize).mockResolvedValue(true);
  vi.mocked(api.beginOAuthAuthorize).mockResolvedValue({
    authorizeUrl: "https://accounts.example.com/authorize", state: "test-state",
    redirectUri: "http://127.0.0.1:1234/oauth/callback",
  });
  vi.mocked(api.completeOAuthAuthorize).mockRejectedValue(new Error("授权失败，请重试"));
  vi.mocked(api.oauthStatus).mockResolvedValue({
    authorized: true, expiresAt: null, scope: null, hasRefreshToken: true,
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("谷歌应用密钥", () => {
  function gmailForm() {
    renderForm();
    fireEvent.change(input("邮箱地址"), { target: { value: "me@gmail.com" } });
    fireEvent.blur(input("邮箱地址"));
    fireEvent.click(screen.getByText("高级设置：客户端编号与应用密钥"));
  }

  it("密钥作为独立参数提交，失败时保留以便重试，不写浏览器存储", async () => {
    gmailForm();
    const secret = input("应用密钥（client_secret）");
    expect(secret.type).toBe("password");
    expect(secret.autocomplete).toBe("off");
    expect(secret.value).toBe("");
    fireEvent.change(secret, { target: { value: "fake-client-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "浏览器授权并保存" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("授权失败"));
    expect(api.beginOAuthAuthorize).toHaveBeenCalledWith(
      expect.objectContaining({ oauthProvider: "gmail" }), undefined, "fake-client-secret",
    );
    const draft = vi.mocked(api.beginOAuthAuthorize).mock.calls[0][0];
    expect(JSON.stringify(draft)).not.toContain("fake-client-secret");
    expect(secret.value).toBe("fake-client-secret");
    expect(JSON.stringify(localStorage)).not.toContain("fake-client-secret");
    expect(JSON.stringify(sessionStorage)).not.toContain("fake-client-secret");
    expect(api.cancelOAuthAuthorize).toHaveBeenCalledWith("test-state");
  });

  it("更换客户端编号或服务商会清空输入，微软不带谷歌密钥", async () => {
    gmailForm();
    fireEvent.change(input("应用密钥（client_secret）"), { target: { value: "first-secret" } });
    fireEvent.change(input("客户端编号（client_id）"), { target: { value: "another-client" } });
    expect(input("应用密钥（client_secret）").value).toBe("");
    fireEvent.change(input("应用密钥（client_secret）"), { target: { value: "second-secret" } });
    fireEvent.change(select("OAuth2 服务商"), { target: { value: "microsoft" } });
    expect(screen.queryByLabelText("应用密钥（client_secret）")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "浏览器授权并保存" }));
    await waitFor(() => expect(api.beginOAuthAuthorize).toHaveBeenCalled());
    expect(api.beginOAuthAuthorize).toHaveBeenCalledWith(
      expect.objectContaining({ oauthProvider: "microsoft" }), undefined, undefined,
    );
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    fireEvent.change(select("OAuth2 服务商"), { target: { value: "gmail" } });
    expect(input("应用密钥（client_secret）").value).toBe("");
  });

  it("已配置的谷歌应用可留空直接授权，不要求重新填写密钥", async () => {
    gmailForm();
    vi.mocked(api.completeOAuthAuthorize).mockResolvedValue({
      account: { id: 9 } as Account,
      report: { imapFolderCount: 3, smtpMechanism: "XOAUTH2" },
    } as OAuthOutcome);
    expect(input("应用密钥（client_secret）").value).toBe("");
    expect(screen.getByText(/谷歌授权会自动使用本机保险箱中已配置的配套应用密钥/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "浏览器授权并保存" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("授权成功"));
    expect(api.beginOAuthAuthorize).toHaveBeenCalledWith(
      expect.objectContaining({ oauthProvider: "gmail", oauthClientId: "" }), undefined, undefined,
    );
  });

  it("授权成功后清空密钥输入", async () => {
    gmailForm();
    vi.mocked(api.completeOAuthAuthorize).mockResolvedValue({
      account: { id: 9 } as Account,
      report: { imapFolderCount: 3, smtpMechanism: "XOAUTH2" },
    } as OAuthOutcome);
    fireEvent.change(input("应用密钥（client_secret）"), { target: { value: "success-secret" } });
    fireEvent.click(screen.getByRole("button", { name: "浏览器授权并保存" }));
    await waitFor(() => expect(screen.getByRole("status").textContent).toContain("授权成功"));
    expect(input("应用密钥（client_secret）").value).toBe("");
  });

  it("已有谷歌账号不回显密钥，留空重新授权不发送密钥参数", async () => {
    const account: Account = {
      id: 8, displayName: "谷歌", email: "me@gmail.com", username: "me@gmail.com",
      authType: "oauth2", oauthProvider: "gmail", oauthClientId: "saved-client",
      imap: { host: "imap.gmail.com", port: 993, security: "tls" },
      smtp: { host: "smtp.gmail.com", port: 465, security: "tls" },
      proxy: { mode: "direct" }, color: "", enabled: true, hasCredential: true,
      createdAt: "", updatedAt: "",
    };
    render(<AccountForm account={account} onSaved={() => {}} onCancel={() => {}} />);
    fireEvent.click(screen.getByText("高级设置：客户端编号与应用密钥"));
    expect(input("应用密钥（client_secret）").value).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "重新授权" }));
    await waitFor(() => expect(api.beginOAuthAuthorize).toHaveBeenCalled());
    expect(api.beginOAuthAuthorize).toHaveBeenCalledWith(expect.any(Object), 8, undefined);
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
  });
});

describe("常用邮箱自动填", () => {
  it("离开邮箱输入框后，直接识别 Foxmail 并补齐服务器参数", () => {
    renderForm();
    const email = input("邮箱地址");

    fireEvent.change(email, { target: { value: "me@foxmail.com" } });
    fireEvent.blur(email);

    expect(input("收件服务器").value).toBe("imap.qq.com");
    expect(input("收件端口").value).toBe("993");
    expect(input("发件服务器").value).toBe("smtp.qq.com");
    expect(input("发件端口").value).toBe("465");
    expect(input("登录名").value).toBe("me@foxmail.com");
    expect(screen.getByText(/已按「Foxmail 邮箱」填入服务器参数/)).toBeTruthy();
  });

  it("按回车也会自动补齐，不触发整张表单提交", () => {
    renderForm();
    const email = input("邮箱地址");

    fireEvent.change(email, { target: { value: "me@vip.qq.com" } });
    fireEvent.keyDown(email, { key: "Enter" });

    expect(input("收件服务器").value).toBe("imap.qq.com");
    expect(input("发件服务器").value).toBe("smtp.qq.com");
    expect(screen.getByText(/已按「QQ 邮箱（VIP）」填入服务器参数/)).toBeTruthy();
  });

  it("用户自己改过服务器参数后，再改邮箱不会覆盖", () => {
    renderForm();
    fireEvent.change(input("收件服务器"), { target: { value: "mail.example.net" } });

    const email = input("邮箱地址");
    fireEvent.change(email, { target: { value: "me@qq.com" } });
    fireEvent.blur(email);

    expect(input("收件服务器").value).toBe("mail.example.net");
    expect(input("发件服务器").value).toBe("smtp.example.com");
  });

  it("填微软邮箱会自动改用 OAuth2 并选好微软服务商", () => {
    renderForm();
    const email = input("邮箱地址");

    fireEvent.change(email, { target: { value: "me@outlook.com" } });
    fireEvent.blur(email);

    expect(select("认证方式").value).toBe("oauth2");
    expect(select("OAuth2 服务商").value).toBe("microsoft");
    expect(input("收件服务器").value).toBe("outlook.office365.com");
    expect(screen.getByText(/自动改用 OAuth2 浏览器授权/)).toBeTruthy();
  });

  it("Hotmail / Live / MSN 也和 Outlook 一样自动切 OAuth2", () => {
    for (const domain of ["hotmail.com", "live.com", "msn.com"]) {
      cleanup();
      renderForm();
      const email = input("邮箱地址");
      fireEvent.change(email, { target: { value: `me@${domain}` } });
      fireEvent.blur(email);
      expect(select("认证方式").value).toBe("oauth2");
      expect(select("OAuth2 服务商").value).toBe("microsoft");
    }
  });

  it("填 Gmail 会自动改用 OAuth2 并选好谷歌服务商", () => {
    renderForm();
    const email = input("邮箱地址");

    fireEvent.change(email, { target: { value: "me@gmail.com" } });
    fireEvent.blur(email);

    expect(select("认证方式").value).toBe("oauth2");
    expect(select("OAuth2 服务商").value).toBe("gmail");
    expect(input("收件服务器").value).toBe("imap.gmail.com");
    expect(input("发件服务器").value).toBe("smtp.gmail.com");
    expect(screen.getByText(/自动改用 OAuth2 浏览器授权/)).toBeTruthy();
  });

  it("用户手动改回授权码登录后，再识别微软邮箱也不覆盖", () => {
    renderForm();
    const email = input("邮箱地址");
    fireEvent.change(email, { target: { value: "me@outlook.com" } });
    fireEvent.blur(email);
    expect(select("认证方式").value).toBe("oauth2");

    fireEvent.change(select("认证方式"), { target: { value: "password" } });
    fireEvent.change(email, { target: { value: "me@hotmail.com" } });
    fireEvent.blur(email);

    expect(select("认证方式").value).toBe("password");
  });

  it("用户手动改回授权码登录后，再识别谷歌邮箱也不覆盖", () => {
    renderForm();
    const email = input("邮箱地址");
    fireEvent.change(email, { target: { value: "me@gmail.com" } });
    fireEvent.blur(email);
    expect(select("认证方式").value).toBe("oauth2");

    fireEvent.change(select("认证方式"), { target: { value: "password" } });
    fireEvent.change(email, { target: { value: "me@gmail.com" } });
    fireEvent.blur(email);

    expect(select("认证方式").value).toBe("password");
  });
});
