//! 常用邮箱自动填回归：离开邮箱框或按回车自动补齐，手工改过的服务器参数不被覆盖。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AccountForm from "../AccountForm";
import { api } from "../api";

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
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
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
