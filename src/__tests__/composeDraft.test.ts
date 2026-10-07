//! 写信草稿纯函数回归：未保存判断、序列化、反序列化。

import { describe, expect, it } from "vitest";

import {
  COMPOSE_DRAFT_STORAGE_KEY,
  deserializeComposeDraft,
  hasUnsavedChanges,
  serializeComposeDraft,
  type ComposeDraftSnapshot,
} from "../composeDraft";

function snapshot(overrides: Partial<ComposeDraftSnapshot> = {}): ComposeDraftSnapshot {
  return {
    accountId: 1,
    toText: "bob@example.com",
    ccText: "",
    bccText: "",
    subject: "季度报告",
    bodyHtml: "<p>正文</p>",
    bodyText: "正文",
    attachments: [],
    signatureOn: false,
    ...overrides,
  };
}

describe("composeDraft 纯函数", () => {
  it("同一份内容判断为没有未保存改动", () => {
    expect(hasUnsavedChanges(snapshot(), snapshot())).toBe(false);
  });

  it("正文、收件人、主题、附件任一变化都算未保存", () => {
    const seed = snapshot();
    expect(hasUnsavedChanges(snapshot({ bodyText: "改了一下" }), seed)).toBe(true);
    expect(hasUnsavedChanges(snapshot({ toText: "alice@example.com" }), seed)).toBe(true);
    expect(hasUnsavedChanges(snapshot({ subject: "新主题" }), seed)).toBe(true);
    expect(
      hasUnsavedChanges(
        snapshot({ attachments: [{ path: "C:/tmp/a.pdf", filename: "a.pdf" }] }),
        seed,
      ),
    ).toBe(true);
  });

  it("没有初始种子时不做未保存判断", () => {
    expect(hasUnsavedChanges(snapshot(), undefined)).toBe(false);
  });

  it("序列化只保留允许的字段，序列化再反序列化结果一致", () => {
    const raw = serializeComposeDraft(
      snapshot({
        attachments: [{ path: "C:/tmp/a.pdf", filename: "a.pdf" }],
        signatureOn: true,
        ccText: "cc@example.com",
      }),
      "new",
      "2026-10-05T00:00:00.000Z",
    );
    const stored = deserializeComposeDraft(raw);
    expect(stored).not.toBeNull();
    expect(stored?.kind).toBe("new");
    expect(stored?.savedAt).toBe("2026-10-05T00:00:00.000Z");
    expect(stored?.toText).toBe("bob@example.com");
    expect(stored?.ccText).toBe("cc@example.com");
    expect(stored?.attachments).toEqual([{ path: "C:/tmp/a.pdf", filename: "a.pdf" }]);
    expect(stored?.signatureOn).toBe(true);
    // 富文本正文要留着，否则恢复出来只剩纯文字；内嵌图片只存编号与路径，不存 base64。
    expect(stored?.bodyHtml).toBe("<p>正文</p>");
    expect(raw).not.toContain("data:image");
    // 凭据类字段一律不进备份。
    expect(raw).not.toContain("password");
    expect(raw).not.toContain("token");
  });

  it("坏数据一律返回 null，不抛异常", () => {
    expect(deserializeComposeDraft(null)).toBeNull();
    expect(deserializeComposeDraft("")).toBeNull();
    expect(deserializeComposeDraft("{不是 json")).toBeNull();
    expect(deserializeComposeDraft("[1,2,3]")).toBeNull();
    expect(deserializeComposeDraft(JSON.stringify({ savedAt: "x" }))).toBeNull();
    expect(deserializeComposeDraft(JSON.stringify({ kind: "new" }))).toBeNull();
  });

  it("反序列化会丢弃类型不对的字段", () => {
    const stored = deserializeComposeDraft(
      JSON.stringify({
        kind: "new",
        savedAt: "2026-10-05T00:00:00.000Z",
        accountId: "1",
        toText: 42,
        subject: null,
        bodyText: "正文",
        attachments: [{ path: "C:/tmp/a.pdf" }, { filename: "b.pdf" }, "bad", null],
        signatureOn: "yes",
      }),
    );
    expect(stored?.accountId).toBeUndefined();
    expect(stored?.toText).toBe("");
    expect(stored?.subject).toBe("");
    expect(stored?.bodyText).toBe("正文");
    expect(stored?.attachments).toEqual([
      { path: "C:/tmp/a.pdf", filename: "" },
      { path: "", filename: "b.pdf" },
    ]);
    expect(stored?.signatureOn).toBe(false);
  });

  it("本地备份键名固定为约定值", () => {
    expect(COMPOSE_DRAFT_STORAGE_KEY).toBe("ymail.compose-draft.v1");
  });
});