//! 段落级格式：缩进与行距（写信工具栏用）。
//!
//! 这两项都挂在段落节点（`paragraph` / `heading`）上，渲染成行内 `style`：
//! 收件人的邮箱客户端拿不到我们的样式表，只有写在标签上的样式才带得走。
//!
//! 注意：TipTap 会把同一个节点上各个属性渲染出的 `style` 合并成一个（同名属性后者覆盖），
//! 所以每个属性只写自己那一个 CSS 属性 —— 缩进只写 `margin-left`，行距只写 `line-height`，
//! 别去碰对方的属性，也别跟对齐抢同一个。

import { Extension, type Editor, type SingleCommands } from "@tiptap/core";

/** 每级缩进的像素数。 */
export const INDENT_UNIT = 24;
/** 最多缩进几级。 */
export const MAX_INDENT = 8;
/** 行距档位：增加 / 减少就在这串数字上前后走一档。 */
export const LINE_HEIGHT_STEPS = [1, 1.15, 1.5, 1.75, 2, 2.5, 3];
/** 没设过行距时按这一档算，第一次点「增加」就从它往上走。 */
export const DEFAULT_LINE_HEIGHT = 1.5;
/** 这两项作用的节点类型。 */
export const BLOCK_TYPES = ["paragraph", "heading"];

declare module "@tiptap/core" {
  interface Commands<ReturnType> {
    blockFormat: {
      /** 段落缩进加一级。 */
      increaseIndent: () => ReturnType;
      /** 段落缩进减一级。 */
      decreaseIndent: () => ReturnType;
      /** 行距加一档。 */
      increaseLineHeight: () => ReturnType;
      /** 行距减一档。 */
      decreaseLineHeight: () => ReturnType;
    };
  }
}

/** 读出光标所在段落的缩进级别。 */
export function currentIndent(editor: Editor): number {
  const raw = Number(editor.state.selection.$from.parent.attrs.indent ?? 0);
  if (!Number.isFinite(raw) || raw <= 0) return 0;
  return Math.min(Math.round(raw), MAX_INDENT);
}

/** 读出光标所在段落的行距；没设过按默认档算。 */
export function currentLineHeight(editor: Editor): number {
  const raw = editor.state.selection.$from.parent.attrs.blockLineHeight;
  const value = Number(raw);
  if (raw === null || raw === undefined || raw === "" || !Number.isFinite(value) || value <= 0) {
    return DEFAULT_LINE_HEIGHT;
  }
  return value;
}

/** 行距上下走一档；已经到头顶或到底就停在原地。 */
export function stepLineHeight(current: number, direction: 1 | -1): number {
  if (direction === 1) {
    return LINE_HEIGHT_STEPS.find((value) => value > current + 1e-6) ?? LINE_HEIGHT_STEPS[LINE_HEIGHT_STEPS.length - 1];
  }
  const smaller = LINE_HEIGHT_STEPS.filter((value) => value < current - 1e-6);
  return smaller.length > 0 ? smaller[smaller.length - 1] : LINE_HEIGHT_STEPS[0];
}

/**
 * 把同一组属性套到段落与标题上。
 *
 * 两个类型都要试着套一次：光标在哪一类段落上就更新哪一类，
 * 不能用 every（另一种类型不存在会直接短路），也不能只看第一个结果。
 */
function applyToBlocks(commands: SingleCommands, attributes: Record<string, unknown>): boolean {
  return BLOCK_TYPES.map((type) => commands.updateAttributes(type, attributes)).some(Boolean);
}

/** 缩进 + 行距的段落格式扩展。 */
export const BlockFormat = Extension.create({
  name: "blockFormat",

  addGlobalAttributes() {
    return [
      {
        types: BLOCK_TYPES,
        attributes: {
          indent: {
            default: 0,
            parseHTML: (element) => {
              const raw = Number(element.getAttribute("data-indent") ?? 0);
              if (!Number.isFinite(raw) || raw <= 0) return 0;
              return Math.min(Math.round(raw), MAX_INDENT);
            },
            renderHTML: (attributes) => {
              const level = Number(attributes.indent ?? 0);
              if (!Number.isFinite(level) || level <= 0) return {};
              return {
                "data-indent": String(level),
                style: `margin-left: ${level * INDENT_UNIT}px`,
              };
            },
          },
          blockLineHeight: {
            default: null,
            parseHTML: (element) => element.style.lineHeight || null,
            renderHTML: (attributes) => {
              const value = attributes.blockLineHeight;
              if (value === null || value === undefined || value === "") return {};
              return { style: `line-height: ${value}` };
            },
          },
        },
      },
    ];
  },

  addCommands() {
    return {
      increaseIndent:
        () =>
        ({ editor, commands }) => {
          const level = currentIndent(editor);
          if (level >= MAX_INDENT) return false;
          return applyToBlocks(commands, { indent: level + 1 });
        },
      decreaseIndent:
        () =>
        ({ editor, commands }) => {
          const level = currentIndent(editor);
          if (level <= 0) return false;
          return applyToBlocks(commands, { indent: level - 1 });
        },
      increaseLineHeight:
        () =>
        ({ editor, commands }) => {
          const next = stepLineHeight(currentLineHeight(editor), 1);
          return applyToBlocks(commands, { blockLineHeight: next });
        },
      decreaseLineHeight:
        () =>
        ({ editor, commands }) => {
          const next = stepLineHeight(currentLineHeight(editor), -1);
          return applyToBlocks(commands, { blockLineHeight: next });
        },
    };
  },
});