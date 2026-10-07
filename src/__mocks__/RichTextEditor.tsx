//! 测试用的假正文编辑器：用多行文本域顶替 TipTap。
//!
//! 写信窗格的测试关心的是发送闸门、草稿恢复这些逻辑，不是编辑器本身；
//! 真编辑器的行为在 richTextEditor.test.tsx 里对着真组件单独测。

interface RichTextEditorProps {
  value: string;
  onChange: (html: string, text: string) => void;
  onAttach: () => void;
  attachments?: { path: string; filename: string }[];
  onRemoveAttachment?: (index: number) => void;
  dropActive?: boolean;
  disabled?: boolean;
}

/** 粗略把 HTML 还原成纯文本，只给测试里比对用。 */
function htmlToText(html: string): string {
  return html
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(p|div|li|h[1-6])>/gi, "\n")
    .replace(/<[^>]+>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .trim();
}

/** 纯文本转成最简单的 HTML，模拟编辑器的输出。 */
function textToHtml(text: string): string {
  const escaped = text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
  return `<p>${escaped.replace(/\r?\n/g, "<br>")}</p>`;
}

export default function MockRichTextEditor({
  value,
  onChange,
  onAttach,
  attachments = [],
  onRemoveAttachment,
  dropActive = false,
  disabled = false,
}: RichTextEditorProps) {
  return (
    <div>
      <button type="button" onClick={onAttach} disabled={disabled}>
        附件
      </button>
      <textarea
        aria-label="正文"
        rows={10}
        value={htmlToText(value)}
        disabled={disabled}
        onChange={(event) => onChange(textToHtml(event.target.value), event.target.value)}
      />
      {attachments.length > 0 && (
        <ul aria-label="已添加的附件">
          {attachments.map((item, index) => (
            <li key={`${item.path}-${index}`}>
              <span>{item.filename || item.path}</span>
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
      <div className={`rte-editor${dropActive ? " drop-active" : ""}`} />
    </div>
  );
}