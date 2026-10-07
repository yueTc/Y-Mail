//! 写信正文收发转换的回归：编辑器里的 data URL 与要发出去的 cid 引用之间来回换。

import { describe, expect, it } from "vitest";

import { htmlForEditor, htmlForSending, newContentId } from "../composeRichText";

describe("正文里的图片转换", () => {
  it("带本地路径的图片换成 cid 引用，并回收路径与编号", () => {
    const html =
      '<p>看图</p><img src="data:image/png;base64,AAAA" data-local-path="C:\\tmp\\a.png" data-content-id="img-1@ymail" alt="a.png" title="a.png">';

    const { html: outgoing, images } = htmlForSending(html);

    expect(outgoing).toContain('src="cid:img-1@ymail"');
    expect(outgoing).not.toContain("data-local-path");
    expect(outgoing).not.toContain("data:image");
    expect(images).toEqual([
      { path: "C:\\tmp\\a.png", filename: "a.png", contentId: "img-1@ymail" },
    ]);
  });

  it("没有本地路径的图片（网图）原样留着，不进内嵌清单", () => {
    const html = '<img src="https://example.com/a.png">';

    const { html: outgoing, images } = htmlForSending(html);

    expect(outgoing).toContain('src="https://example.com/a.png"');
    expect(images).toEqual([]);
  });

  it("编号统一小写，免得和邮件侧的规范化结果对不上", () => {
    const { images } = htmlForSending(
      '<img src="data:image/png;base64,AA" data-local-path="C:/tmp/a.png" data-content-id="IMG-1@YMAIL">',
    );

    expect(images[0]?.contentId).toBe("img-1@ymail");
  });

  it("恢复草稿时把 cid 换回 data URL，并写回路径属性", () => {
    const draft = '<p>看图</p><img src="cid:img-1@ymail" alt="a.png">';

    const restored = htmlForEditor(draft, [
      {
        contentId: "img-1@ymail",
        path: "C:/tmp/a.png",
        filename: "a.png",
        dataUrl: "data:image/png;base64,AAAA",
      },
    ]);

    expect(restored).toContain('src="data:image/png;base64,AAAA"');
    expect(restored).toContain('data-local-path="C:/tmp/a.png"');
    expect(restored).toContain('data-content-id="img-1@ymail"');
    // 再存一次仍然能换回 cid 引用，来回不丢。
    expect(htmlForSending(restored).images[0]?.path).toBe("C:/tmp/a.png");
  });

  it("认不出来的 cid 原样保留，不硬塞假图", () => {
    const draft = '<img src="cid:找不到@ymail">';

    const restored = htmlForEditor(draft, []);

    expect(restored).toContain('src="cid:找不到@ymail"');
  });

  it("生成的编号只含小写安全字符", () => {
    const id = newContentId();
    expect(id).toMatch(/^[a-z0-9._@+-]+$/);
  });
});