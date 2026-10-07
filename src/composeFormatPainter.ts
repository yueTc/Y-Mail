//! 写信工具栏的「格式刷」与「清除格式」。
//!
//! 两者都只动格式，不动正文结构：列表还是列表，表格还是表格，图片还在原地。
//! 全部在本地编辑器里完成，不联网、不碰正文内容。

import type { Editor } from "@tiptap/core";

import { BLOCK_TYPES } from "./composeBlockFormat";

/** 清除格式时保留的标记。链接不算「格式」—— 清掉就把网址变成死文本了。 */
export const KEEP_MARKS = ["link"];

/** 段落级格式属性：清格式时一起复位，格式刷时整套替换。 */
export const BLOCK_FORMAT_ATTRS = ["textAlign", "indent", "blockLineHeight"];

/** 从某处刷走的一套格式。 */
export interface PickedFormat {
  /** 字符格式：标记名 + 属性（加粗、颜色、字体、字号……）。 */
  marks: { type: string; attrs: Record<string, unknown> }[];
  /** 段落格式：对齐、缩进、行距。 */
  block: Record<string, unknown>;
}

/**
 * 读出光标 / 选区起点处的格式。
 *
 * 选了一段就从这段的起点取；只放了个光标就按当前位置的格式取。
 */
export function captureFormat(editor: Editor): PickedFormat {
  const { state } = editor;
  const { selection } = state;
  const marks = state.storedMarks ?? selection.$from.marks();
  const attrs = selection.$from.parent.attrs;
  return {
    marks: marks.map((mark) => ({ type: mark.type.name, attrs: { ...mark.attrs } })),
    block: {
      textAlign: attrs.textAlign ?? null,
      indent: attrs.indent ?? 0,
      blockLineHeight: attrs.blockLineHeight ?? null,
    },
  };
}

/** 选区里出现过的标记名；光标没有选区时按当前位置的标记算。 */
export function markTypesInSelection(editor: Editor): string[] {
  const { state } = editor;
  const { from, to } = state.selection;
  const names = new Set<string>();
  state.doc.nodesBetween(from, to, (node) => {
    if (node.isText) {
      for (const mark of node.marks) names.add(mark.type.name);
    }
  });
  if (names.size === 0) {
    for (const mark of state.storedMarks ?? state.selection.$from.marks()) {
      names.add(mark.type.name);
    }
  }
  return [...names];
}

/**
 * 把一套格式刷到当前选区上。
 *
 * 先把选区上原有的字符格式清掉再套上这套，避免和目标位置的旧格式叠在一起；
 * 段落格式（对齐 / 缩进 / 行距）整套替换。
 */
export function applyFormat(editor: Editor, format: PickedFormat): void {
  const chain = editor.chain().focus().unsetAllMarks();
  for (const mark of format.marks) {
    chain.setMark(mark.type, mark.attrs);
  }
  for (const type of BLOCK_TYPES) {
    chain.updateAttributes(type, format.block);
  }
  chain.run();
}

/**
 * 清除选区的格式。
 *
 * 去掉字符格式与段落格式（对齐 / 缩进 / 行距），保留链接；
 * 不动结构，所以列表、表格、图片都还在。
 */
export function clearFormat(editor: Editor): void {
  const chain = editor.chain().focus();
  for (const name of markTypesInSelection(editor)) {
    if (!KEEP_MARKS.includes(name)) chain.unsetMark(name);
  }
  chain.unsetTextAlign();
  for (const type of BLOCK_TYPES) {
    chain.resetAttributes(type, BLOCK_FORMAT_ATTRS);
  }
  chain.run();
}