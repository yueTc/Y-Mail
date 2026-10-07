//! 格式刷与清除格式的纯逻辑回归：对着真编辑器验，不经过界面。
//!
//! 界面上那两个按钮怎么点、怎么失效在 richTextEditor.test.tsx 里测；
//! 这里只管「格式抓得对不对、刷得对不对、清得干不干净」。

import { Editor } from "@tiptap/core";
import { afterEach, describe, expect, it } from "vitest";

import { applyFormat, captureFormat, clearFormat } from "../composeFormatPainter";
import { composeEditorExtensions } from "../RichTextEditor";

let editors: Editor[] = [];

/** 造一个真编辑器；用完在 afterEach 里统一销毁。 */
function makeEditor(content: string): Editor {
  const element = document.createElement("div");
  document.body.appendChild(element);
  const editor = new Editor({
    element,
    extensions: composeEditorExtensions(),
    content,
  });
  editors.push(editor);
  return editor;
}

/** 取第 n 块的 HTML，免得断言被别的块「蹭」过去。 */
function blockHtml(editor: Editor, index: number): string {
  const parsed = new DOMParser().parseFromString(editor.getHTML(), "text/html");
  return parsed.body.children[index]?.outerHTML ?? "";
}

/** 找到一段文字在文档里的位置，省得手算坐标。 */
function findTextRange(editor: Editor, text: string): { from: number; to: number } {
  let found: { from: number; to: number } | null = null;
  editor.state.doc.descendants((node, pos) => {
    if (found || !node.isText || !node.text) return;
    const index = node.text.indexOf(text);
    if (index >= 0) {
      found = { from: pos + index, to: pos + index + text.length };
    }
  });
  if (!found) throw new Error(`文档里没有这段文字：${text}`);
  return found;
}

afterEach(() => {
  for (const editor of editors) editor.destroy();
  editors = [];
});

describe("格式刷", () => {
  it("把源文字的加粗与颜色刷到目标文字上", () => {
    const editor = makeEditor(
      '<p><strong><span style="color: #ff0000">红的</span></strong></p><p>普通</p>',
    );
    editor.commands.setTextSelection(findTextRange(editor, "红的"));
    const picked = captureFormat(editor);

    editor.commands.setTextSelection(findTextRange(editor, "普通"));
    applyFormat(editor, picked);

    const target = blockHtml(editor, 1);
    expect(target).toContain("<strong>普通</strong>");
    expect(target).toContain("rgb(255, 0, 0)");
  });

  it("段落格式（对齐、缩进）一起刷过来", () => {
    const editor = makeEditor('<p data-indent="2" style="text-align: center">源</p><p>目标</p>');
    editor.commands.setTextSelection(findTextRange(editor, "源"));
    const picked = captureFormat(editor);

    editor.commands.setTextSelection(findTextRange(editor, "目标"));
    applyFormat(editor, picked);

    const target = blockHtml(editor, 1);
    expect(target).toContain("text-align: center");
    expect(target).toContain("margin-left: 48px");
  });
});

describe("清除格式", () => {
  it("去掉字符与段落格式，链接和列表结构都留着", () => {
    const editor = makeEditor(
      '<p data-indent="2" style="text-align: center; line-height: 2"><strong>粗</strong>' +
        '<span style="color: #ff0000">红</span><a href="https://example.com">链</a></p>' +
        "<ul><li><p>项目</p></li></ul>",
    );
    editor.commands.selectAll();
    clearFormat(editor);

    const html = editor.getHTML();
    expect(html).not.toContain("<strong>");
    expect(html).not.toContain("color:");
    expect(html).not.toContain("text-align");
    expect(html).not.toContain("margin-left");
    expect(html).not.toContain("line-height");
    expect(html).toContain('href="https://example.com"');
    expect(html).toContain("<ul>");
  });
});