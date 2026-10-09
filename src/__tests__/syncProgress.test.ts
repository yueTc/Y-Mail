//! 同步总量进度的文字：总数已知时报「已同步 XX 封 / 共 XX 封」。

import { describe, expect, it } from "vitest";

import type { SyncStatus } from "../api";
import { progressText } from "../SyncPanel";

function status(patch: Partial<SyncStatus>): SyncStatus {
  return {
    accountId: 1,
    email: "a@example.com",
    state: "idle_waiting",
    stateLabel: "等待新邮件",
    progress: 0,
    total: 0,
    message: "",
    needsReauth: false,
    updatedAt: "",
    ...patch,
  };
}

describe("同步总量进度", () => {
  it("总数已知时报已同步多少、共多少", () => {
    expect(progressText(status({ progress: 1200, total: 20000 }))).toBe(
      "已同步 1200 封 / 共 20000 封",
    );
  });

  it("只有进度没有总数时只报已同步多少", () => {
    expect(progressText(status({ progress: 30 }))).toBe("已同步 30 封");
  });

  it("什么都没统计到时不报数字", () => {
    expect(progressText(status({}))).toBe("");
  });
});