//! 读信设置面板：列出「记住的发件人」并支持移除（移除后恢复默认拦截）。

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ReaderSettingsPanel from "../ReaderSettingsPanel";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listTrustedRemoteSenders: vi.fn(),
    forgetRemoteSender: vi.fn(),
  },
}));

beforeEach(() => {
  vi.mocked(api.listTrustedRemoteSenders).mockResolvedValue(["a@example.com", "b@example.com"]);
  vi.mocked(api.forgetRemoteSender).mockResolvedValue(["b@example.com"]);
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("读信设置的记住发件人", () => {
  it("列出记住的发件人", async () => {
    render(<ReaderSettingsPanel />);
    await screen.findByText("a@example.com");
    expect(screen.getByText("b@example.com")).toBeTruthy();
    expect(api.listTrustedRemoteSenders).toHaveBeenCalled();
  });

  it("移除后从列表消失并恢复默认拦截", async () => {
    render(<ReaderSettingsPanel />);
    await screen.findByText("a@example.com");
    fireEvent.click(screen.getAllByRole("button", { name: "移除" })[0]);
    await vi.waitFor(() => expect(api.forgetRemoteSender).toHaveBeenCalledWith("a@example.com"));
    await vi.waitFor(() => expect(screen.queryByText("a@example.com")).toBeNull());
    expect(screen.getByText("b@example.com")).toBeTruthy();
  });

  it("没有记住发件人时给出提示", async () => {
    vi.mocked(api.listTrustedRemoteSenders).mockResolvedValue([]);
    render(<ReaderSettingsPanel />);
    await screen.findByText(/还没有记住的发件人/);
  });
});