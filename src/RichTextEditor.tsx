//! 写信正文的富文本编辑器（TipTap）。
//!
//! 这一批工具栏只做用户点的五项：附件、插入（图片 / 截图 / 表格 / 链接）、字体、字号、颜色。
//! 加粗、斜体、下划线、对齐、缩进这些先不做按钮（快捷键仍在，TipTap 自带）。
//!
//! 图片一律走本地文件：编辑器里显示 data URL，存草稿和发信时换成 `cid:` 引用，
//! 真正的字节由邮件侧按路径读（见 composeRichText.ts）。

import Image from "@tiptap/extension-image";
import { Table, TableCell, TableHeader, TableRow } from "@tiptap/extension-table";
import TextAlign from "@tiptap/extension-text-align";
import {
  BackgroundColor,
  Color,
  FontFamily,
  FontSize,
  TextStyle,
} from "@tiptap/extension-text-style";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import StarterKit from "@tiptap/starter-kit";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useRef, useState } from "react";

import { api, describeError, type InlineImageInfo } from "./api";
import { BlockFormat } from "./composeBlockFormat";
import {
  applyFormat,
  captureFormat,
  clearFormat,
  type PickedFormat,
} from "./composeFormatPainter";
import { IMAGE_CID_ATTR, IMAGE_PATH_ATTR, newContentId } from "./composeRichText";

/** 截图完成后主窗口收到的事件名；与后端 compose_images.rs 一致。 */
const SCREENSHOT_EVENT = "compose:screenshot-ready";
/** 最近使用颜色的本地存储键。 */
const RECENT_COLOR_KEY = "ymail.compose-recent-colors.v1";
/** 颜色面板里默认的当前色（参考图里 A 下面那条红线）。 */
const DEFAULT_COLOR = "#C00000";
/** 背景色默认值（面板里第一行的黄，按钮上那个色块用它）。 */
const DEFAULT_BACKGROUND = "#FFC000";
/** 选图片时放行的扩展名。 */
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif"];

/** 字体候选：显示名 + 写进正文的 CSS 值。 */
export const FONT_CHOICES: { label: string; value: string }[] = [
  { label: "默认", value: "" },
  { label: "微软雅黑", value: "微软雅黑" },
  { label: "宋体", value: "宋体" },
  { label: "黑体", value: "黑体" },
  { label: "楷体", value: "楷体" },
  { label: "仿宋", value: "仿宋" },
  { label: "等线", value: "等线" },
  { label: "Arial", value: "Arial, sans-serif" },
  { label: "Times New Roman", value: "'Times New Roman', serif" },
  { label: "Courier New", value: "'Courier New', monospace" },
  { label: "Verdana", value: "Verdana, sans-serif" },
];

/** 字号候选（像素）。 */
export const SIZE_CHOICES = [12, 14, 16, 18, 20, 24, 28, 32, 36, 48];

/**
 * 颜色面板：第一行是标准色，下面四行是同一批色系的明暗档。
 * 行列形状照参考图：5 行 × 9 列。
 */
export const COLOR_ROWS: string[][] = [
  ["#FFFFFF", "#000000", "#FF0000", "#ED7D31", "#FFC000", "#92D050", "#00B0F0", "#002060", "#7030A0"],
  ["#F2F2F2", "#808080", "#F8CBAD", "#FFE699", "#FFF2CC", "#E2EFDA", "#DEEBF7", "#D9E2F3", "#E4DFEC"],
  ["#D9D9D9", "#595959", "#F4B183", "#FFD966", "#FFE599", "#C6E0B4", "#BDD7EE", "#B4C6E7", "#CCC0DA"],
  ["#BFBFBF", "#404040", "#ED7D31", "#BF9000", "#D6B656", "#A9D08E", "#9DC3E6", "#8EA9DB", "#B4A7D6"],
  ["#A6A6A6", "#262626", "#C55A11", "#7F6000", "#9E7E1E", "#548235", "#2E75B6", "#2F5597", "#8064A2"],
];

/** 「最近使用」最多记几个。 */
const RECENT_LIMIT = 8;

/** 图片节点多带两个属性：本地路径与内嵌编号，保存时要靠它们还原。 */
const LocalImage = Image.extend({
  addAttributes() {
    return {
      ...this.parent?.(),
      localPath: {
        default: null,
        parseHTML: (element: Element) => element.getAttribute(IMAGE_PATH_ATTR),
        renderHTML: (attributes: Record<string, unknown>) =>
          attributes.localPath ? { [IMAGE_PATH_ATTR]: attributes.localPath } : {},
      },
      contentId: {
        default: null,
        parseHTML: (element: Element) => element.getAttribute(IMAGE_CID_ATTR),
        renderHTML: (attributes: Record<string, unknown>) =>
          attributes.contentId ? { [IMAGE_CID_ATTR]: attributes.contentId } : {},
      },
    };
  },
});

/** 链接只放行这几种开头的写法，挡掉 javascript: 这类危险协议。 */
export function normalizeHref(raw: string): string | null {
  const value = raw.trim();
  if (value === "") return null;
  const lower = value.toLowerCase();
  if (lower.startsWith("http://") || lower.startsWith("https://") || lower.startsWith("mailto:")) {
    return value;
  }
  // 只写了 domain.com 这种，默认补 https。
  if (/^[\w-]+(\.[\w-]+)+([/?#].*)?$/.test(value)) return `https://${value}`;
  return null;
}

/** 读最近使用的颜色。 */
function readRecentColors(): string[] {
  try {
    const raw = window.localStorage.getItem(RECENT_COLOR_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((item): item is string => typeof item === "string" && item.trim() !== "")
      .slice(0, RECENT_LIMIT);
  } catch {
    return [];
  }
}

/** 写最近使用的颜色；写不进去就只影响这次会话。 */
function writeRecentColors(colors: string[]) {
  try {
    window.localStorage.setItem(RECENT_COLOR_KEY, JSON.stringify(colors));
  } catch {
    // 本地存储不可用时忽略。
  }
}

/** 读一个图片文件成 data URL。 */
function fileToDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result ?? ""));
    reader.onerror = () => reject(new Error("读取剪贴板里的图片失败"));
    reader.readAsDataURL(file);
  });
}

/**
 * 编辑器用到的扩展清单。
 *
 * 单独抽出来是因为测试要另造一个编辑器对着真扩展验格式逻辑，
 * 两边必须用同一份，否则测出来的和界面上跑的是两套。
 */
export function composeEditorExtensions() {
  return [
    StarterKit.configure({
      link: { openOnClick: false, autolink: true },
    }),
    TextStyle,
    FontFamily,
    FontSize,
    Color,
    LocalImage.configure({ allowBase64: true }),
    TextAlign.configure({ types: ["heading", "paragraph"] }),
    BackgroundColor,
    BlockFormat,
    Table.configure({ resizable: false }),
    TableRow,
    TableHeader,
    TableCell,
  ];
}

/** 格式刷图标（内联矢量图，不引图标库；名字靠按钮上的 title 显示）。 */
function FormatPainterIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M15.5 3.5 20.5 8.5 12.5 16.5 7.5 11.5z" />
        <path d="M7.5 11.5 5 14a2.5 2.5 0 0 0 0 3.5L7.5 20 12 15.5" />
      </g>
    </svg>
  );
}

/** 清除格式图标：一个带斜杠的 A。 */
function ClearFormatIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" focusable="false">
      <g
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M4 20h16" />
        <path d="M7 16.5 11.5 5.5 16 16.5" />
        <path d="M8.8 13h5.4" />
        <path d="M4.5 4.5 18 18" />
      </g>
    </svg>
  );
}

/** 颜色面板：文字颜色与背景色共用同一套色块和「最近使用」。 */
function ColorPanel({
  namePrefix,
  current,
  recent,
  onPick,
}: {
  namePrefix: string;
  current: string;
  recent: string[];
  onPick: (color: string) => void;
}) {
  return (
    <div className="rte-menu rte-colors" role="menu" aria-label={namePrefix}>
      {COLOR_ROWS.map((row, rowIndex) => (
        <div className="rte-color-row" key={rowIndex}>
          {row.map((color) => (
            <button
              key={`${rowIndex}-${color}`}
              type="button"
              role="menuitem"
              aria-label={`${namePrefix} ${color}`}
              className={`rte-swatch${
                color.toLowerCase() === current.toLowerCase() ? " active" : ""
              }`}
              style={{ background: color }}
              onClick={() => onPick(color)}
            />
          ))}
        </div>
      ))}
      <p className="rte-recent-title">最近使用</p>
      <div className="rte-color-row">
        {Array.from({ length: RECENT_LIMIT }).map((_, index) => {
          const color = recent[index];
          return (
            <button
              key={`recent-${index}`}
              type="button"
              role="menuitem"
              aria-label={color ? `最近使用 ${color}` : `最近使用 ${index + 1}`}
              className="rte-swatch"
              style={{ background: color ?? "transparent" }}
              disabled={!color}
              onClick={() => color && onPick(color)}
            />
          );
        })}
      </div>
    </div>
  );
}

interface RichTextEditorProps {
  /** 正文 HTML（显示形态，图片是 data URL）。 */
  value: string;
  /** 内容变化：HTML + 纯文本。 */
  onChange: (html: string, text: string) => void;
  /** 点工具栏「附件」。 */
  onAttach: () => void;
  /** 已经挂上的附件（工具栏选的和拖进来的），列在正文上方。 */
  attachments?: { path: string; filename: string }[];
  /** 移除第几个附件。 */
  onRemoveAttachment?: (index: number) => void;
  /** 有文件正拖在写信窗格上：正文区高亮，告诉用户可以松手。 */
  dropActive?: boolean;
  /** 只读（发送中）。 */
  disabled?: boolean;
}

type MenuKind = "insert" | "font" | "size" | "color" | "background" | null;

/** 写信正文编辑器。 */
export default function RichTextEditor({
  value,
  onChange,
  onAttach,
  attachments = [],
  onRemoveAttachment,
  dropActive = false,
  disabled = false,
}: RichTextEditorProps) {
  const [menu, setMenu] = useState<MenuKind>(null);
  const [tableOpen, setTableOpen] = useState(false);
  const [tableRows, setTableRows] = useState(3);
  const [tableCols, setTableCols] = useState(3);
  const [linkOpen, setLinkOpen] = useState(false);
  const [linkText, setLinkText] = useState("");
  const [linkHref, setLinkHref] = useState("");
  const [recentColors, setRecentColors] = useState<string[]>(readRecentColors);
  const [currentColor, setCurrentColor] = useState(DEFAULT_COLOR);
  const [currentBackground, setCurrentBackground] = useState(DEFAULT_BACKGROUND);
  const [error, setError] = useState("");
  const [brush, setBrush] = useState<PickedFormat | null>(null);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const editorRef = useRef<Editor | null>(null);
  const brushAnchorRef = useRef<{ from: number; to: number } | null>(null);
  const lastHtmlRef = useRef(value);

  /** 把一张已经读好的图片插到光标处。 */
  const insertImage = useCallback((info: InlineImageInfo, contentId = newContentId()) => {
    const editor = editorRef.current;
    if (!editor) return;
    editor
      .chain()
      .focus()
      .insertContent({
        type: "image",
        attrs: {
          src: info.dataUrl,
          alt: info.filename,
          title: info.filename,
          localPath: info.path,
          contentId,
        },
      })
      .run();
  }, []);

  /** 粘贴进来的图片：先落盘拿到本地路径，再插进正文。 */
  const insertPastedImages = useCallback(
    async (files: File[]) => {
      for (const file of files) {
        try {
          const dataUrl = await fileToDataUrl(file);
          const info = await api.saveInlineImage(dataUrl);
          insertImage(info);
        } catch (caught) {
          setError(describeError(caught));
        }
      }
    },
    [insertImage],
  );

  const editor = useEditor({
    extensions: composeEditorExtensions(),
    content: value,
    editable: !disabled,
    editorProps: {
      attributes: {
        class: "rte-content",
        "aria-label": "正文",
      },
      handlePaste: (_view, event) => {
        const files = Array.from(event.clipboardData?.files ?? []).filter((file) =>
          file.type.startsWith("image/"),
        );
        if (files.length === 0) return false;
        event.preventDefault();
        void insertPastedImages(files);
        return true;
      },
    },
    onUpdate: ({ editor: current }) => {
      const html = current.getHTML();
      lastHtmlRef.current = html;
      onChange(html, current.getText({ blockSeparator: "\n\n" }));
    },
  });

  /** 编辑器实例随时同步到 ref，事件回调和粘贴都用它。 */
  useEffect(() => {
    editorRef.current = editor ?? null;
  }, [editor]);

  /** 外部改了内容（AI 润色、恢复草稿）时同步进编辑器。 */
  useEffect(() => {
    if (!editor) return;
    if (value === lastHtmlRef.current) return;
    lastHtmlRef.current = value;
    editor.commands.setContent(value, { emitUpdate: false });
  }, [editor, value]);

  /** 发送中禁止编辑。 */
  useEffect(() => {
    editor?.setEditable(!disabled);
  }, [editor, disabled]);

  /** 截图回来以后把图片插进正文。 */
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let stopped = false;
    void listen<InlineImageInfo>(SCREENSHOT_EVENT, (event) => {
      if (event.payload) insertImage(event.payload);
    })
      .then((stop) => {
        if (stopped) stop();
        else unlisten = stop;
      })
      .catch(() => {
        // 拿不到事件通道只是截图插不进来，不影响继续写信。
      });
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [insertImage]);

  /** 点空白处收起下拉。 */
  useEffect(() => {
    if (!menu) return;
    const handle = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setMenu(null);
    };
    document.addEventListener("mousedown", handle);
    return () => document.removeEventListener("mousedown", handle);
  }, [menu]);

  /** 选字体的值；空串表示回到默认。 */
  const applyFont = useCallback((value: string) => {
    const editor = editorRef.current;
    if (!editor) return;
    if (value === "") editor.chain().focus().unsetFontFamily().run();
    else editor.chain().focus().setFontFamily(value).run();
    setMenu(null);
  }, []);

  /** 选字号；空串表示回到默认。 */
  const applySize = useCallback((value: string) => {
    const editor = editorRef.current;
    if (!editor) return;
    if (value === "") editor.chain().focus().unsetFontSize().run();
    else editor.chain().focus().setFontSize(`${value}px`).run();
    setMenu(null);
  }, []);

  /** 选颜色，并记进「最近使用」。 */
  const applyColor = useCallback((color: string) => {
    const editor = editorRef.current;
    if (!editor) return;
    editor.chain().focus().setColor(color).run();
    setCurrentColor(color);
    setRecentColors((old) => {
      const next = [color, ...old.filter((item) => item.toLowerCase() !== color.toLowerCase())].slice(
        0,
        RECENT_LIMIT,
      );
      writeRecentColors(next);
      return next;
    });
    setMenu(null);
  }, []);

  /** 选背景色，并记进「最近使用」。 */
  const applyBackground = useCallback((color: string) => {
    const editor = editorRef.current;
    if (!editor) return;
    editor.chain().focus().setBackgroundColor(color).run();
    setCurrentBackground(color);
    setRecentColors((old) => {
      const next = [color, ...old.filter((item) => item.toLowerCase() !== color.toLowerCase())].slice(
        0,
        RECENT_LIMIT,
      );
      writeRecentColors(next);
      return next;
    });
    setMenu(null);
  }, []);

  /**
   * 点格式刷：把当前格式记下来；再点一次取消。
   *
   * 记下当时的选区位置，之后只有选区真的变了才刷 —— 否则点按钮那一下
   * 自己触发的事件就把格式刷掉了。
   */
  const toggleFormatBrush = useCallback(() => {
    const editor = editorRef.current;
    setBrush((current) => {
      if (current) {
        brushAnchorRef.current = null;
        return null;
      }
      if (!editor) return null;
      const { from, to } = editor.state.selection;
      brushAnchorRef.current = { from, to };
      return captureFormat(editor);
    });
  }, []);

  /** 选区定下来（松鼠标 / 抬按键）就把格式刷上去；刷一次即失效，和 Word 一样。 */
  const applyFormatBrush = useCallback(() => {
    const editor = editorRef.current;
    if (!brush || !editor) return;
    const anchor = brushAnchorRef.current;
    const { from, to } = editor.state.selection;
    if (anchor && anchor.from === from && anchor.to === to) return;
    brushAnchorRef.current = null;
    setBrush(null);
    applyFormat(editor, brush);
  }, [brush]);

  /** 清除选区的格式（不动列表、表格、图片，链接也留着）。 */
  const runClearFormat = useCallback(() => {
    const editor = editorRef.current;
    if (editor) clearFormat(editor);
  }, []);

  /** 从资源管理器选图片。 */
  const pickImage = useCallback(async () => {
    setMenu(null);
    setError("");
    try {
      const chosen = await open({
        multiple: true,
        title: "选择图片",
        filters: [{ name: "图片", extensions: IMAGE_EXTENSIONS }],
      });
      if (!chosen) return;
      const paths = Array.isArray(chosen) ? chosen : [chosen];
      for (const path of paths) {
        const info = await api.readInlineImage(path);
        insertImage(info);
      }
    } catch (caught) {
      setError(describeError(caught));
    }
  }, [insertImage]);

  /** 进截图：主窗口会先藏起来，截完由事件把图片送回来。 */
  const startScreenshot = useCallback(async () => {
    setMenu(null);
    setError("");
    try {
      await api.openScreenshotOverlay();
    } catch (caught) {
      setError(describeError(caught));
    }
  }, []);

  /** 插入表格。 */
  const confirmTable = useCallback(() => {
    const editor = editorRef.current;
    if (!editor) return;
    const rows = Math.min(Math.max(Math.round(tableRows), 1), 30);
    const cols = Math.min(Math.max(Math.round(tableCols), 1), 12);
    editor.chain().focus().insertTable({ rows, cols, withHeaderRow: true }).run();
    setTableOpen(false);
  }, [tableCols, tableRows]);

  /** 插入链接。 */
  const confirmLink = useCallback(() => {
    const editor = editorRef.current;
    if (!editor) return;
    const href = normalizeHref(linkHref);
    if (!href) {
      setError("链接要以 http://、https:// 或 mailto: 开头");
      return;
    }
    const label = linkText.trim() || href;
    editor
      .chain()
      .focus()
      .insertContent({
        type: "text",
        text: label,
        marks: [{ type: "link", attrs: { href } }],
      })
      .run();
    setError("");
    setLinkText("");
    setLinkHref("");
    setLinkOpen(false);
  }, [linkHref, linkText]);

  return (
    <div className="rte" ref={rootRef}>
      <div className="rte-toolbar" role="toolbar" aria-label="正文工具栏">
        <button type="button" onClick={onAttach} disabled={disabled} title="从文件管理器选择附件">
          附件
        </button>

        <div className="rte-menu-wrap">
          <button
            type="button"
            disabled={disabled}
            aria-expanded={menu === "insert"}
            onClick={() => setMenu(menu === "insert" ? null : "insert")}
          >
            插入 ▾
          </button>
          {menu === "insert" && (
            <div className="rte-menu" role="menu" aria-label="插入">
              <button type="button" role="menuitem" onClick={() => void pickImage()}>
                图片
              </button>
              <button type="button" role="menuitem" onClick={() => void startScreenshot()}>
                截图
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenu(null);
                  setTableOpen(true);
                }}
              >
                表格
              </button>
              <button
                type="button"
                role="menuitem"
                onClick={() => {
                  setMenu(null);
                  setLinkOpen(true);
                }}
              >
                链接
              </button>
            </div>
          )}
        </div>

        <div className="rte-menu-wrap">
          <button
            type="button"
            disabled={disabled}
            aria-expanded={menu === "font"}
            onClick={() => setMenu(menu === "font" ? null : "font")}
          >
            字体 ▾
          </button>
          {menu === "font" && (
            <div className="rte-menu" role="menu" aria-label="字体">
              {FONT_CHOICES.map((item) => (
                <button
                  key={item.label}
                  type="button"
                  role="menuitem"
                  style={{ fontFamily: item.value || "inherit" }}
                  onClick={() => applyFont(item.value)}
                >
                  {item.label}
                </button>
              ))}
            </div>
          )}
        </div>

        <div className="rte-menu-wrap">
          <button
            type="button"
            disabled={disabled}
            aria-expanded={menu === "size"}
            onClick={() => setMenu(menu === "size" ? null : "size")}
          >
            字号 ▾
          </button>
          {menu === "size" && (
            <div className="rte-menu" role="menu" aria-label="字号">
              <button type="button" role="menuitem" onClick={() => applySize("")}>
                默认
              </button>
              {SIZE_CHOICES.map((size) => (
                <button key={size} type="button" role="menuitem" onClick={() => applySize(String(size))}>
                  {size}
                </button>
              ))}
            </div>
          )}
        </div>

        <div className="rte-menu-wrap">
          <button
            type="button"
            className="rte-color-button"
            aria-label="颜色"
            disabled={disabled}
            aria-expanded={menu === "color"}
            onClick={() => setMenu(menu === "color" ? null : "color")}
          >
            <span className="rte-color-letter" style={{ borderBottomColor: currentColor }}>
              A
            </span>
            <span className="rte-caret">▾</span>
          </button>
          {menu === "color" && (
            <ColorPanel
              namePrefix="颜色"
              current={currentColor}
              recent={recentColors}
              onPick={applyColor}
            />
          )}
        </div>

        <div className="rte-menu-wrap">
          <button
            type="button"
            className="rte-color-button"
            aria-label="背景色"
            disabled={disabled}
            aria-expanded={menu === "background"}
            onClick={() => setMenu(menu === "background" ? null : "background")}
          >
            <span className="rte-highlight-letter" style={{ background: currentBackground }}>
              A
            </span>
            <span className="rte-caret">▾</span>
          </button>
          {menu === "background" && (
            <ColorPanel
              namePrefix="背景色"
              current={currentBackground}
              recent={recentColors}
              onPick={applyBackground}
            />
          )}
        </div>

        <span className="rte-sep" aria-hidden="true" />

        <button
          type="button"
          className={editor?.isActive("bold") ? "is-active" : undefined}
          aria-label="加粗"
          aria-pressed={editor?.isActive("bold") ?? false}
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleBold().run()}
        >
          <strong>B</strong>
        </button>
        <button
          type="button"
          className={editor?.isActive("italic") ? "is-active" : undefined}
          aria-label="斜体"
          aria-pressed={editor?.isActive("italic") ?? false}
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleItalic().run()}
        >
          <em>I</em>
        </button>
        <button
          type="button"
          className={editor?.isActive("underline") ? "is-active" : undefined}
          aria-label="下划线"
          aria-pressed={editor?.isActive("underline") ?? false}
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleUnderline().run()}
        >
          <u>U</u>
        </button>
        <button
          type="button"
          className={editor?.isActive("strike") ? "is-active" : undefined}
          aria-label="删除线"
          aria-pressed={editor?.isActive("strike") ?? false}
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleStrike().run()}
        >
          <s>S</s>
        </button>

        <span className="rte-sep" aria-hidden="true" />

        <button
          type="button"
          className={editor?.isActive({ textAlign: "left" }) ? "is-active" : undefined}
          aria-label="居左"
          disabled={disabled}
          onClick={() => editor?.chain().focus().setTextAlign("left").run()}
        >
          居左
        </button>
        <button
          type="button"
          className={editor?.isActive({ textAlign: "center" }) ? "is-active" : undefined}
          aria-label="居中"
          disabled={disabled}
          onClick={() => editor?.chain().focus().setTextAlign("center").run()}
        >
          居中
        </button>
        <button
          type="button"
          className={editor?.isActive({ textAlign: "right" }) ? "is-active" : undefined}
          aria-label="居右"
          disabled={disabled}
          onClick={() => editor?.chain().focus().setTextAlign("right").run()}
        >
          居右
        </button>

        <span className="rte-sep" aria-hidden="true" />

        <button
          type="button"
          className={editor?.isActive("bulletList") ? "is-active" : undefined}
          aria-label="项目符号列表"
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleBulletList().run()}
        >
          • 列表
        </button>
        <button
          type="button"
          className={editor?.isActive("orderedList") ? "is-active" : undefined}
          aria-label="数字编号列表"
          disabled={disabled}
          onClick={() => editor?.chain().focus().toggleOrderedList().run()}
        >
          1. 列表
        </button>

        <span className="rte-sep" aria-hidden="true" />

        <button
          type="button"
          aria-label="减少缩进"
          disabled={disabled}
          onClick={() => editor?.chain().focus().decreaseIndent().run()}
        >
          缩进－
        </button>
        <button
          type="button"
          aria-label="增加缩进"
          disabled={disabled}
          onClick={() => editor?.chain().focus().increaseIndent().run()}
        >
          缩进＋
        </button>
        <button
          type="button"
          aria-label="减少行距"
          disabled={disabled}
          onClick={() => editor?.chain().focus().decreaseLineHeight().run()}
        >
          行距－
        </button>
        <button
          type="button"
          aria-label="增加行距"
          disabled={disabled}
          onClick={() => editor?.chain().focus().increaseLineHeight().run()}
        >
          行距＋
        </button>

        <span className="rte-sep" aria-hidden="true" />

        <button
          type="button"
          className={`rte-icon-button${brush ? " is-active" : ""}`}
          aria-label="格式刷"
          title="格式刷"
          aria-pressed={brush !== null}
          disabled={disabled}
          onClick={toggleFormatBrush}
        >
          <FormatPainterIcon />
        </button>
        <button
          type="button"
          className="rte-icon-button"
          aria-label="清除格式"
          title="清除格式"
          disabled={disabled}
          onClick={runClearFormat}
        >
          <ClearFormatIcon />
        </button>
      </div>

      {attachments.length > 0 && (
        <ul className="rte-attachments" aria-label="已添加的附件">
          {attachments.map((item, index) => (
            <li key={`${item.path}-${index}`}>
              <span className="rte-attachment-name">{item.filename || item.path}</span>
              <button
                type="button"
                aria-label={`移除附件 ${item.filename}`}
                disabled={disabled}
                onClick={() => onRemoveAttachment?.(index)}
              >
                移除
              </button>
            </li>
          ))}
        </ul>
      )}

      <EditorContent
        editor={editor}
        className={`rte-editor${dropActive ? " drop-active" : ""}${brush ? " brush-active" : ""}`}
        onMouseUp={applyFormatBrush}
        onKeyUp={applyFormatBrush}
      />

      {error && <p className="error rte-error">操作失败：{error}</p>}

      {tableOpen && (
        <div className="ai-modal-backdrop" role="presentation">
          <section className="ai-modal" role="dialog" aria-modal="true" aria-label="插入表格">
            <h3>插入表格</h3>
            <label className="compose-field">
              <span>几行</span>
              <input
                type="number"
                min={1}
                max={30}
                aria-label="表格行数"
                value={tableRows}
                onChange={(event) => setTableRows(Number(event.target.value))}
              />
            </label>
            <label className="compose-field">
              <span>几列</span>
              <input
                type="number"
                min={1}
                max={12}
                aria-label="表格列数"
                value={tableCols}
                onChange={(event) => setTableCols(Number(event.target.value))}
              />
            </label>
            <div className="form-actions">
              <button type="button" className="primary" onClick={confirmTable}>
                插入
              </button>
              <button type="button" onClick={() => setTableOpen(false)}>
                取消
              </button>
            </div>
          </section>
        </div>
      )}

      {linkOpen && (
        <div className="ai-modal-backdrop" role="presentation">
          <section className="ai-modal" role="dialog" aria-modal="true" aria-label="插入链接">
            <h3>插入链接</h3>
            <label className="compose-field">
              <span>链接文字</span>
              <input
                aria-label="链接文字"
                value={linkText}
                placeholder="留空就用网址本身"
                onChange={(event) => setLinkText(event.target.value)}
              />
            </label>
            <label className="compose-field">
              <span>链接地址</span>
              <input
                aria-label="链接地址"
                value={linkHref}
                placeholder="https://example.com"
                onChange={(event) => setLinkHref(event.target.value)}
              />
            </label>
            <div className="form-actions">
              <button type="button" className="primary" onClick={confirmLink}>
                插入
              </button>
              <button type="button" onClick={() => setLinkOpen(false)}>
                取消
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}