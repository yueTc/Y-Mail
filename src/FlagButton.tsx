//! 红旗按钮：列表行和读信页标题区共用。
//!
//! 未标红是空心 `☆`，标红是红色实心 `★`。按钮可键盘聚焦，回车或空格触发；
//! 点击和回车都会阻止事件冒泡，避免连带把整行邮件打开。

import { t } from "./i18n";

/** 红旗按钮属性。 */
export interface FlagButtonProps {
  /** 当前是否已标红。 */
  flagged: boolean;
  /** 点击切换；传入目标状态由调用方决定。 */
  onToggle: () => void;
  /** 无障碍标签；默认按状态给「标红 / 取消标红」。 */
  label?: string;
  /** 附加类名，便于读信页调整位置。 */
  className?: string;
}

/** 列表与读信页共用的红旗切换按钮。 */
export default function FlagButton({ flagged, onToggle, label, className }: FlagButtonProps) {
  const title = flagged ? t("取消标红") : t("标红");
  const text = flagged ? "★" : "☆";
  const classes = ["flag-button", flagged ? "flagged" : "", className ?? ""]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type="button"
      className={classes}
      aria-pressed={flagged}
      aria-label={label ?? title}
      title={title}
      onClick={(event) => {
        event.stopPropagation();
        onToggle();
      }}
      onKeyDown={(event) => {
        // 回车 / 空格交给浏览器触发 click；拦住冒泡避免整行也跟着响应。
        if (event.key === "Enter" || event.key === " ") event.stopPropagation();
      }}
    >
      {text}
    </button>
  );
}