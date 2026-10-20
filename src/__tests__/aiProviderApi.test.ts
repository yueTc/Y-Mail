import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";

import { api, type AccountDraft } from "../api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockResolvedValue(["relay/jev-custom"]);
});

const oauthDraft: AccountDraft = {
  displayName: "谷歌", email: "me@gmail.com", username: "me@gmail.com",
  authType: "oauth2", oauthProvider: "gmail", oauthClientId: "client-test",
  imap: { host: "imap.gmail.com", port: 993, security: "tls" },
  smtp: { host: "smtp.gmail.com", port: 465, security: "tls" },
  proxy: { mode: "direct" }, enabled: true, color: "",
};

it("授权命令将应用密钥与普通账号配置分开传递", async () => {
  await api.beginOAuthAuthorize(oauthDraft, 8, "fake-client-secret");
  expect(invoke).toHaveBeenCalledWith("begin_oauth_authorize", {
    draft: oauthDraft, accountId: 8, clientSecret: "fake-client-secret",
  });
  expect(JSON.stringify(oauthDraft)).not.toContain("fake-client-secret");
});

it("不填密钥时授权命令不新增密钥参数，兼容旧调用", async () => {
  await api.beginOAuthAuthorize(oauthDraft);
  expect(invoke).toHaveBeenCalledWith("begin_oauth_authorize", { draft: oauthDraft });
});

it("测试连接命令完整传递所选中转模型、站点和代理", async () => {
  const result = await api.testAiProvider(
    "typesafe", "https://relay.example.com/v1", undefined, 7, { mode: "direct" }, "relay/jev-custom",
  );
  expect(invoke).toHaveBeenCalledWith("test_ai_provider", {
    kind: "typesafe",
    baseUrl: "https://relay.example.com/v1",
    id: 7,
    proxy: { mode: "direct" },
    model: "relay/jev-custom",
  });
  expect(result).toEqual(["relay/jev-custom"]);
});

it("通用决策站点测试连接传递完整地址和模型", async () => {
  const result = await api.testAiProvider(
    "decision_api",
    "https://api.bamboonode.dev/v1/decisions",
    undefined,
    9,
    { mode: "direct" },
    "startlux-decision-27b-autothink",
  );
  expect(invoke).toHaveBeenCalledWith("test_ai_provider", {
    kind: "decision_api",
    baseUrl: "https://api.bamboonode.dev/v1/decisions",
    id: 9,
    proxy: { mode: "direct" },
    model: "startlux-decision-27b-autothink",
  });
  expect(result).toEqual(["relay/jev-custom"]);
});

it("未选测试模型的兼容站点调用保持原有参数", async () => {
  await api.testAiProvider("openai_compatible", "https://relay.example.com/v1");
  expect(invoke).toHaveBeenCalledWith("test_ai_provider", {
    kind: "openai_compatible",
    baseUrl: "https://relay.example.com/v1",
  });
});
