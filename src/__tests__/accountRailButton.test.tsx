import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import AccountRailButton from "../AccountRailButton";
import type { GitHubLoginView } from "../api";

const profile: GitHubLoginView = {
  login: "octocat", name: "八爪猫", scope: "login",
  avatarUrl: "https://avatars.githubusercontent.com/u/1",
};

afterEach(cleanup);

describe("主导航账号图标", () => {
  it("未登录时显示人形，点击打开同步入口", () => {
    const onClick = vi.fn();
    render(<AccountRailButton profile={null} onClick={onClick} />);
    const button = screen.getByRole("button", { name: "GitHub 账号" });
    expect(button.querySelector("svg")).toBeTruthy();
    expect(screen.queryByRole("img")).toBeNull();
    fireEvent.click(button);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it("本机缓存优先，退出后恢复人形", () => {
    const cached = "data:image/png;base64,iVBORw0KGgo=";
    const { rerender } = render(
      <AccountRailButton profile={{ ...profile, avatarDataUrl: cached }} onClick={() => {}} />,
    );
    expect(screen.getByRole("img").getAttribute("src")).toBe(cached);
    rerender(<AccountRailButton profile={null} onClick={() => {}} />);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("button").querySelector("svg")).toBeTruthy();
  });

  it("无缓存时使用合法头像，加载失败后退回人形", () => {
    render(<AccountRailButton profile={profile} onClick={() => {}} />);
    const avatar = screen.getByRole("img");
    expect(avatar.getAttribute("src")).toBe(profile.avatarUrl);
    expect(avatar.getAttribute("referrerpolicy")).toBe("no-referrer");
    fireEvent.error(avatar);
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("button").querySelector("svg")).toBeTruthy();
  });

  it("不加载其他域名或非图片缓存", () => {
    render(
      <AccountRailButton
        profile={{
          ...profile,
          avatarUrl: "https://example.com/avatar.png",
          avatarDataUrl: "data:text/html;base64,PHNjcmlwdD4=",
        }}
        onClick={() => {}}
      />,
    );
    expect(screen.queryByRole("img")).toBeNull();
    expect(screen.getByRole("button").querySelector("svg")).toBeTruthy();
  });
});
