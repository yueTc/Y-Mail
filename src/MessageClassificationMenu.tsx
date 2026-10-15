//! 邮件的本地手动分类菜单：入口 → 标签组 → 标签或分数。

import { useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import type { LabelGroup } from "./api";
import { t } from "./i18n";
import { scoreOptions } from "./labelPill";

/** 每层使用独立浮层，避免被父菜单的滚动区域裁掉。 */
function ClassificationSubmenu({ name, children, active = true, onActivate }: {
  name: string;
  children: ReactNode;
  active?: boolean;
  onActivate?: () => void;
}) {
  const [requestedOpen, setOpen] = useState(false);
  const open = requestedOpen && active;
  const [position, setPosition] = useState({ left: 0, top: 0, side: "right" });
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const focusOnOpen = useRef(false);
  const id = useId();

  const openWithFocus = () => {
    onActivate?.();
    focusOnOpen.current = true;
    setOpen(true);
    panelRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  };

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !panelRef.current) return;
    const anchor = triggerRef.current.getBoundingClientRect();
    const panel = panelRef.current.getBoundingClientRect();
    // 上层向左展开时，本层继续向左，避免盖住最初的邮件菜单。
    const preferLeft = triggerRef.current.closest<HTMLElement>(".classification-submenu")?.dataset.side === "left";
    const fitsLeft = anchor.left - panel.width >= 8;
    const fitsRight = anchor.right + panel.width <= window.innerWidth - 8;
    const side = (preferLeft && fitsLeft) || !fitsRight ? "left" : "right";
    const left = side === "left" ? anchor.left - panel.width : anchor.right;
    setPosition({
      left: Math.max(8, Math.min(left, window.innerWidth - panel.width - 8)),
      top: Math.max(8, Math.min(anchor.top - 4, window.innerHeight - panel.height - 8)),
      side,
    });
    if (focusOnOpen.current) {
      focusOnOpen.current = false;
      panelRef.current.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    }
    // 父菜单滚动后收起本层，避免浮层继续指向旧位置；本层自己仍可滚动。
    const onScroll = (event: Event) => {
      if (event.target instanceof HTMLElement && event.target.contains(triggerRef.current)) {
        setOpen(false);
      }
    };
    document.addEventListener("scroll", onScroll, true);
    return () => document.removeEventListener("scroll", onScroll, true);
  }, [open, children]);

  return (
    <div
      className="classification-menu-branch"
      onMouseLeave={() => setOpen(false)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        ref={triggerRef}
        type="button"
        role="menuitem"
        className="context-menu-item"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onMouseEnter={() => { onActivate?.(); setOpen(true); }}
        onClick={openWithFocus}
        onKeyDown={(event) => {
          if (event.key === "ArrowRight") {
            event.preventDefault();
            event.stopPropagation();
            openWithFocus();
          }
        }}
      >
        <span className="classification-menu-name" title={name}>{name}</span>
        <svg className="classification-menu-arrow" width="14" height="14" viewBox="0 0 16 16" fill="none" aria-hidden="true">
          <path d="m6 4 4 4-4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open && (
        <div
          ref={panelRef}
          id={id}
          role="menu"
          aria-label={name}
          className="context-menu classification-submenu"
          data-side={position.side}
          style={{ left: position.left, top: position.top }}
          onKeyDown={(event) => {
            if (event.key === "ArrowLeft") {
              event.preventDefault();
              event.stopPropagation();
              setOpen(false);
              triggerRef.current?.focus();
            } else if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
              event.preventDefault();
              event.stopPropagation();
              const items = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>(
                ":scope > button:not(:disabled), :scope > .classification-menu-branch > button",
              ));
              const current = items.indexOf(document.activeElement as HTMLButtonElement);
              const next = event.key === "Home" ? 0
                : event.key === "End" ? items.length - 1
                : (current + (event.key === "ArrowUp" ? -1 : 1) + items.length) % items.length;
              items[next]?.focus();
            }
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
}

export default function MessageClassificationMenu({
  groups, loading, error, onSetValue, onClearValue,
}: {
  groups: LabelGroup[];
  loading: boolean;
  error: boolean;
  onSetValue: (groupId: number, labelId: number | null, scoreValue: number | null) => void;
  onClearValue: (groupId: number) => void;
}) {
  const [activeGroup, setActiveGroup] = useState<number>();
  return (
    <ClassificationSubmenu name={t("手动分类")}>
      {loading || error || groups.length === 0 ? (
        <div className="classification-menu-note" role="status">
          {loading ? t("加载中……") : error ? t("标签组读取失败，请重新打开菜单") : t("暂无标签组")}
        </div>
      ) : groups.map((group) => (
        <ClassificationSubmenu key={group.id} name={group.name}
          active={activeGroup === group.id} onActivate={() => setActiveGroup(group.id)}>
          {group.kind === "score"
            ? scoreOptions(group.scoreMin, group.scoreMax).map((value) => (
                <button key={value} type="button" role="menuitem" className="context-menu-item"
                  onClick={() => onSetValue(group.id, null, value)}>{value}</button>
              ))
            : group.labels.map((label) => (
                <button key={label.id} type="button" role="menuitem" className="context-menu-item"
                  title={label.name} onClick={() => onSetValue(group.id, label.id, null)}>
                  <span className="classification-menu-name">{label.name}</span>
                </button>
              ))}
          <div role="separator" className="classification-menu-separator" />
          <button type="button" role="menuitem" className="context-menu-item"
            onClick={() => onClearValue(group.id)}>{t("清除本组结果")}</button>
        </ClassificationSubmenu>
      ))}
    </ClassificationSubmenu>
  );
}
