//! MCP 面板前端回归：默认显示「MCP 未启用」，启用后显示工具清单与写工具风险说明，
//! 工具清单里不能出现发送类工具。

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import McpPanel from "../McpPanel";
import type { McpStatus, McpTool } from "../api";
import { api } from "../api";

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    mcpStatus: vi.fn(),
    mcpTools: vi.fn(),
    mcpAudit: vi.fn(),
    mcpSetEnabled: vi.fn(),
    mcpSetWriteTools: vi.fn(),
  },
}));

const disabledStatus: McpStatus = {
  enabled: false,
  writeToolsEnabled: false,
  dataDir: "C:/Users/tester/AppData/Roaming/com.emmaster.desktop",
  binaryName: "em-master-mcp",
  dataDirEnv: "EM_MASTER_DATA_DIR",
  protocolVersions: ["2025-06-18", "2025-03-26", "2024-11-05"],
  configExample: "{\n  \"mcpServers\": {}\n}",
};

const enabledStatus: McpStatus = { ...disabledStatus, enabled: true };

const tools: McpTool[] = [
  {
    name: "search_messages",
    title: "搜索邮件",
    description: "在本机已同步的邮件里检索。",
    readOnly: true,
    enabled: true,
  },
  {
    name: "create_draft",
    title: "建草稿",
    description: "把一封新邮件存成草稿；不会发送。",
    readOnly: false,
    enabled: false,
  },
];

let current: McpStatus;

beforeEach(() => {
  current = disabledStatus;
  (api.mcpStatus as ReturnType<typeof vi.fn>).mockImplementation(async () => current);
  (api.mcpTools as ReturnType<typeof vi.fn>).mockImplementation(async () =>
    tools.map((tool) => ({ ...tool, enabled: current.enabled && (tool.readOnly || current.writeToolsEnabled) })),
  );
  (api.mcpAudit as ReturnType<typeof vi.fn>).mockResolvedValue({ items: [], total: 0 });
  (api.mcpSetEnabled as ReturnType<typeof vi.fn>).mockImplementation(async (enabled: boolean) => {
    current = enabled ? enabledStatus : disabledStatus;
    return current;
  });
  (api.mcpSetWriteTools as ReturnType<typeof vi.fn>).mockImplementation(async (enabled: boolean) => {
    current = { ...current, writeToolsEnabled: enabled };
    return current;
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("MCP 外部接入面板", () => {
  it("默认显示 MCP 未启用与启用说明", async () => {
    render(<McpPanel />);
    expect(await screen.findByText(/MCP 未启用/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "启用 MCP" })).toBeTruthy();
    expect(screen.queryByText("send_email")).toBeNull();
  });

  it("启用后显示工具清单、写工具风险说明与配置示例", async () => {
    render(<McpPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "启用 MCP" }));

    await waitFor(() => expect(api.mcpSetEnabled).toHaveBeenCalledWith(true));
    expect(await screen.findByText(/MCP 已启用/)).toBeTruthy();
    expect(screen.getByText("search_messages")).toBeTruthy();
    expect(screen.getByText("create_draft")).toBeTruthy();
    expect(screen.getAllByText(/不会发送/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/EM_MASTER_DATA_DIR/).length).toBeGreaterThan(0);
    expect(screen.queryByText("send_email")).toBeNull();
    expect(screen.queryByText(/export_all/)).toBeNull();
  });

  it("打开写工具会调用独立开关", async () => {
    render(<McpPanel />);
    fireEvent.click(await screen.findByRole("button", { name: "启用 MCP" }));
    const writeButton = await screen.findByRole("button", { name: "打开写工具" });
    fireEvent.click(writeButton);
    await waitFor(() => expect(api.mcpSetWriteTools).toHaveBeenCalledWith(true));
  });
});