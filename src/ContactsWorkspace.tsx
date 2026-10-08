//! 通讯录：三栏地址簿（分组栏 / 列表栏 / 详情栏）。
//!
//! 口径（见 `docs/superpowers/specs/2026-10-06-contacts-workspace-design.md` v1.2）：
//! - 一份共用的通讯录，一个邮箱一条；删除 = 隐藏，进「已隐藏」，可恢复；
//! - 左侧分组栏：搜索 + 分组列表（所有联系人 / 未分组 / 自建分组 / 已隐藏）；
//! - 中间列表栏：按数字区与拼音首字母分区排序，可批量操作；
//! - 右侧详情栏：看详情，也在同一栏里就地编辑；
//! - 导入导出只用用户在系统对话框里亲手选的本地文件，不联网。

import { open, save } from "@tauri-apps/plugin-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
} from "react";

import {
  api,
  describeError,
  type Contact,
  type ContactCounts,
  type ContactDraft,
  type ContactGroup,
  type ContactImportPreview,
} from "./api";
import PaneResizer from "./PaneResizer";
import { CONTACTS_PANE_PROFILE, RESIZER_WIDTH, usePaneWidths } from "./usePaneWidths";
import { t } from "./i18n";

/** 一次最多拉多少条；与后端上限一致。 */
const PAGE_LIMIT = 5000;

/** 列表行高：分区标题矮一些。 */
const SECTION_ROW_HEIGHT = 30;
const CONTACT_ROW_HEIGHT = 58;

type ContactsWorkspaceProps = {
  /** 点「写邮件」时把联系人交给外壳，由外壳切回收件箱并预填收件人。 */
  onCompose: (contact: Contact) => void;
};

/** 左侧当前选中哪一项。 */
export type ContactsView =
  | { kind: "all" }
  | { kind: "ungrouped" }
  | { kind: "hidden" }
  | { kind: "group"; id: number };

/** 右键菜单：要么开在分组上，要么开在联系人上。 */
type ContactsMenu =
  | { kind: "group"; group: ContactGroup; x: number; y: number }
  | { kind: "contact"; contact: Contact; x: number; y: number };

/** 列表里的一行：分区标题，或一位联系人。 */
export type ContactsListRow =
  | { kind: "section"; key: string; label: string }
  | { kind: "contact"; key: string; contact: Contact };

/** 联系人的显示名；没名字就用邮箱顶，不留空行。 */
export function contactLabel(contact: Contact): string {
  const name = contact.name.trim();
  return name === "" ? contact.email : name;
}

/** 按名字或邮箱做本地包含过滤；关键字为空返回全部。 */
export function filterContacts(contacts: Contact[], keyword: string): Contact[] {
  const needle = keyword.trim().toLowerCase();
  if (needle === "") return contacts;
  return contacts.filter(
    (contact) =>
      contact.name.toLowerCase().includes(needle) ||
      contact.email.toLowerCase().includes(needle),
  );
}

/** 最近联系时间：只给到「哪天」，空值说明还没正式来往过。 */
export function lastUsedLabel(iso: string | null): string {
  if (!iso) return t("还没通过信");
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return t("还没通过信");
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  return t("{0}-{1}-{2} 联系过", [date.getFullYear(), month, day]);
}

let pinyinCollator: Intl.Collator | undefined;

/** 排序用的比较器：中文按拼音，数字按数值。 */
export function contactsCollator(): Intl.Collator {
  pinyinCollator ??= new Intl.Collator("zh-Hans-CN", { numeric: true, sensitivity: "base" });
  return pinyinCollator;
}

/**
 * 拼音首字母的参照字。
 *
 * 浏览器自带的排序本来就按拼音走，这里只是拿一组「每个声母最靠前的常用字」
 * 去二分，把名字落到 A–Z 的哪个桶里。多音字和生僻字可能分错桶，
 * 但**顺序不会错**——排序交给同一个比较器。分不出来就落到「#」。
 */
const PINYIN_ANCHORS: ReadonlyArray<readonly [string, string]> = [
  ["A", "阿"],
  ["B", "芭"],
  ["C", "擦"],
  ["D", "搭"],
  ["E", "蛾"],
  ["F", "发"],
  ["G", "噶"],
  ["H", "哈"],
  ["J", "击"],
  ["K", "喀"],
  ["L", "垃"],
  ["M", "妈"],
  ["N", "拿"],
  ["O", "哦"],
  ["P", "啪"],
  ["Q", "七"],
  ["R", "然"],
  ["S", "撒"],
  ["T", "塌"],
  ["W", "挖"],
  ["X", "昔"],
  ["Y", "压"],
  ["Z", "匝"],
];

/** 一位联系人落在哪个分区：数字区 / A–Z / 其他区。 */
export function contactBucket(contact: Contact): string {
  const first = Array.from(contactLabel(contact).trim())[0];
  if (!first) return "#";
  if (first >= "0" && first <= "9") return "0-9";
  if (/[a-zA-Z]/.test(first)) return first.toUpperCase();
  const collator = contactsCollator();
  let letter = "#";
  for (const [name, anchor] of PINYIN_ANCHORS) {
    if (collator.compare(anchor, first) <= 0) letter = name;
    else break;
  }
  return letter;
}

/** 分区排序权重：数字区最前，然后 A–Z，最后其他区。 */
function bucketRank(bucket: string): number {
  if (bucket === "0-9") return 0;
  if (bucket.length === 1 && bucket >= "A" && bucket <= "Z") {
    return bucket.charCodeAt(0) - 64;
  }
  return 27;
}

/** 按分区与拼音顺序排好。 */
export function sortContacts(contacts: Contact[]): Contact[] {
  const collator = contactsCollator();
  return [...contacts].sort((left, right) => {
    const rank = bucketRank(contactBucket(left)) - bucketRank(contactBucket(right));
    if (rank !== 0) return rank;
    const byName = collator.compare(contactLabel(left), contactLabel(right));
    if (byName !== 0) return byName;
    return collator.compare(left.email, right.email);
  });
}

/** 把排好序的联系人摊成「分区标题 + 联系人」的行，供虚拟列表用。 */
export function buildListRows(contacts: Contact[]): ContactsListRow[] {
  const rows: ContactsListRow[] = [];
  let current = "";
  for (const contact of contacts) {
    const bucket = contactBucket(contact);
    if (bucket !== current) {
      current = bucket;
      rows.push({ kind: "section", key: `section-${bucket}`, label: bucket });
    }
    rows.push({ kind: "contact", key: `contact-${contact.id}`, contact });
  }
  return rows;
}

/** 当前视图的标题。 */
export function viewTitle(view: ContactsView, groups: ContactGroup[]): string {
  if (view.kind === "all") return t("所有联系人");
  if (view.kind === "ungrouped") return t("未分组");
  if (view.kind === "hidden") return t("已隐藏");
  return groups.find((group) => group.id === view.id)?.name ?? t("分组");
}

/** 空草稿。 */
function emptyDraft(groupId: number | null): ContactDraft {
  return { name: "", email: "", note: "", groupId };
}

/** 把联系人转成编辑用的草稿。 */
function draftOf(contact: Contact): ContactDraft {
  return {
    name: contact.name,
    email: contact.email,
    note: contact.note,
    groupId: contact.groupId,
  };
}

/** 通讯录三栏页。 */
export default function ContactsWorkspace({ onCompose }: ContactsWorkspaceProps) {
  const [view, setView] = useState<ContactsView>({ kind: "all" });
  const [keyword, setKeyword] = useState("");
  const [contacts, setContacts] = useState<Contact[]>([]);
  const [hiddenContacts, setHiddenContacts] = useState<Contact[]>([]);
  const [groups, setGroups] = useState<ContactGroup[]>([]);
  const [counts, setCounts] = useState<ContactCounts>({ active: 0, hidden: 0, ungrouped: 0 });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState("");
  const [version, setVersion] = useState(0);

  const [selectedId, setSelectedId] = useState<number>();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<ContactDraft>(emptyDraft(null));
  const [saving, setSaving] = useState(false);

  const [batchMode, setBatchMode] = useState(false);
  const [checked, setChecked] = useState<Set<number>>(new Set());

  const [menu, setMenu] = useState<ContactsMenu>();
  const [submenuOpen, setSubmenuOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [importPreview, setImportPreview] = useState<ContactImportPreview>();
  const [importPath, setImportPath] = useState("");
  const [importOverwrite, setImportOverwrite] = useState(false);
  const [importBusy, setImportBusy] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  const reload = useCallback(() => setVersion((value) => value + 1), []);

  const {
    widths,
    limits: paneLimits,
    setSidebar,
    setList,
    reset: resetPaneWidths,
    persist: persistPaneWidths,
  } = usePaneWidths(undefined, CONTACTS_PANE_PROFILE);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    Promise.all([
      api.listContacts("", PAGE_LIMIT, "active"),
      api.listContacts("", PAGE_LIMIT, "hidden"),
      api.listContactGroups(),
      api.contactCounts(),
    ])
      .then(([active, hidden, groupList, snapshot]) => {
        if (cancelled) return;
        setContacts(active);
        setHiddenContacts(hidden);
        setGroups(groupList);
        setCounts(snapshot);
        setError(undefined);
      })
      .catch((caught: unknown) => {
        if (!cancelled) setError(describeError(caught));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [version]);

  /** 当前视图底下的原始列表。 */
  const source = view.kind === "hidden" ? hiddenContacts : contacts;

  const visible = useMemo(() => {
    let list = filterContacts(source, keyword);
    if (view.kind === "ungrouped") list = list.filter((item) => item.groupId === null);
    else if (view.kind === "group") {
      const groupId = view.id;
      list = list.filter((item) => item.groupId === groupId);
    }
    return sortContacts(list);
  }, [source, keyword, view]);

  const rows = useMemo(() => buildListRows(visible), [visible]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) =>
      rows[index]?.kind === "section" ? SECTION_ROW_HEIGHT : CONTACT_ROW_HEIGHT,
    overscan: 12,
  });

  const selected = useMemo(
    () => source.find((item) => item.id === selectedId),
    [source, selectedId],
  );

  /** 一句短提示，几秒后自己消失。 */
  const flash = useCallback((message: string) => {
    setNotice(message);
  }, []);

  useEffect(() => {
    if (notice === "") return;
    const handle = setTimeout(() => setNotice(""), 4000);
    return () => clearTimeout(handle);
  }, [notice]);

  /** 跑一个改数据的动作，成功就刷新，失败就把原因原样说出来。 */
  const runAction = useCallback(
    async (action: () => Promise<void>, done?: string) => {
      try {
        await action();
        if (done) flash(done);
        reload();
      } catch (caught: unknown) {
        flash(describeError(caught));
      }
    },
    [flash, reload],
  );

  const startCreate = useCallback(() => {
    const groupId = view.kind === "group" ? view.id : null;
    setSelectedId(undefined);
    setDraft(emptyDraft(groupId));
    setEditing(true);
  }, [view]);

  const startEdit = useCallback((contact: Contact) => {
    setSelectedId(contact.id);
    setDraft(draftOf(contact));
    setEditing(true);
  }, []);

  const cancelEdit = useCallback(() => {
    setEditing(false);
    setDraft(emptyDraft(null));
  }, []);

  const saveDraft = useCallback(async () => {
    setSaving(true);
    try {
      if (selectedId === undefined) {
        await api.createContact(draft);
        flash(t("联系人已新建"));
      } else {
        await api.updateContact(selectedId, draft);
        flash(t("联系人已保存"));
      }
      setEditing(false);
      setDraft(emptyDraft(null));
      reload();
    } catch (caught: unknown) {
      // 校验没过时留在表单里，用户敲的东西一个都不丢。
      flash(describeError(caught));
    } finally {
      setSaving(false);
    }
  }, [draft, flash, reload, selectedId]);

  const hideContact = useCallback(
    (contact: Contact) => {
      if (!window.confirm(t("把「{0}」移到已隐藏？", [contactLabel(contact)]))) return;
      void runAction(async () => {
        await api.hideContact(contact.id);
        if (selectedId === contact.id) setSelectedId(undefined);
      }, t("已移到「已隐藏」"));
    },
    [runAction, selectedId],
  );

  const restoreContact = useCallback(
    (contact: Contact) => {
      void runAction(async () => {
        await api.restoreContact(contact.id);
        if (selectedId === contact.id) setSelectedId(undefined);
      }, t("已恢复"));
    },
    [runAction, selectedId],
  );

  const purgeContact = useCallback(
    (contact: Contact) => {
      const tip =
        t("彻底删除后，以后要是再收到这个地址的来信，它会重新出现在通讯录里。确定要删吗？");
      if (!window.confirm(`${contactLabel(contact)}\n\n${tip}`)) return;
      void runAction(async () => {
        await api.purgeContact(contact.id);
        if (selectedId === contact.id) setSelectedId(undefined);
      }, t("已彻底删除"));
    },
    [runAction, selectedId],
  );

  const copyAddress = useCallback(
    async (contact: Contact) => {
      try {
        await navigator.clipboard.writeText(contact.email);
        flash(t("地址已复制"));
      } catch {
        flash(t("复制失败，请手动选中"));
      }
    },
    [flash],
  );

  /** 右键菜单：开在分组上。 */
  const openGroupMenu = useCallback((event: ReactMouseEvent, group: ContactGroup) => {
    event.preventDefault();
    event.stopPropagation();
    setSubmenuOpen(false);
    setMenu({ kind: "group", group, x: event.clientX, y: event.clientY });
  }, []);

  /** 右键菜单：开在联系人上。 */
  const openContactMenu = useCallback((event: ReactMouseEvent, contact: Contact) => {
    event.preventDefault();
    event.stopPropagation();
    setSubmenuOpen(false);
    setMenu({ kind: "contact", contact, x: event.clientX, y: event.clientY });
  }, []);

  const closeMenu = useCallback(() => {
    setMenu(undefined);
    setSubmenuOpen(false);
  }, []);

  // 菜单开着的时候，点别处 / 按 Esc / 一滚动就收起来。
  useEffect(() => {
    if (!menu) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeMenu();
    };
    document.addEventListener("click", closeMenu);
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("scroll", closeMenu, true);
    return () => {
      document.removeEventListener("click", closeMenu);
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("scroll", closeMenu, true);
    };
  }, [closeMenu, menu]);

  /** 把一位联系人挪到某个分组（null = 未分组）。 */
  const moveOneToGroup = useCallback(
    async (contact: Contact, groupId: number | null) => {
      const name = groupId === null ? t("未分组") : groups.find((item) => item.id === groupId)?.name ?? t("分组");
      await runAction(async () => {
        await api.updateContact(contact.id, {
          name: contact.name,
          email: contact.email,
          note: contact.note,
          groupId,
        });
      }, t("已移到「{0}」", [name]));
    },
    [groups, runAction],
  );

  const createGroup = useCallback(async () => {
    const name = window.prompt(t("新分组叫什么？"));
    if (name === null) return;
    await runAction(async () => {
      await api.createContactGroup(name);
    }, t("分组已新建"));
  }, [runAction]);

  const renameGroup = useCallback(
    async (group: ContactGroup) => {
      const name = window.prompt(t("改成什么名字？"), group.name);
      if (name === null) return;
      await runAction(async () => {
        await api.renameContactGroup(group.id, name);
      }, t("分组已改名"));
    },
    [runAction],
  );

  const removeGroup = useCallback(
    async (group: ContactGroup) => {
      if (!window.confirm(t("删掉分组「{0}」？组内联系人会回到「未分组」，联系人本身不会被删。", [group.name]))) {
        return;
      }
      await runAction(async () => {
        await api.deleteContactGroup(group.id);
        if (view.kind === "group" && view.id === group.id) setView({ kind: "all" });
      }, t("分组已删除"));
    },
    [runAction, view],
  );

  /** 批量隐藏。 */
  const hideChecked = useCallback(() => {
    const list = contacts.filter((item) => checked.has(item.id));
    if (list.length === 0) return;
    if (!window.confirm(t("把选中的 {0} 位移到「已隐藏」？", [list.length]))) return;
    void runAction(async () => {
      for (const item of list) await api.hideContact(item.id);
      setChecked(new Set());
      setBatchMode(false);
      setSelectedId(undefined);
    }, t("已隐藏 {0} 位", [list.length]));
  }, [checked, contacts, runAction]);

  /** 批量改分组。 */
  const moveChecked = useCallback(
    async (groupId: number | null) => {
      const list = contacts.filter((item) => checked.has(item.id));
      if (list.length === 0) return;
      await runAction(async () => {
        for (const item of list) {
          await api.updateContact(item.id, {
            name: item.name,
            email: item.email,
            note: item.note,
            groupId,
          });
        }
        setChecked(new Set());
        setBatchMode(false);
      }, t("已移动 {0} 位", [list.length]));
    },
    [checked, contacts, runAction],
  );

  /** 导出到用户选定的文件。 */
  const exportContacts = useCallback(
    async (kind: "csv" | "vcf") => {
      setExportOpen(false);
      const extension = kind === "csv" ? "csv" : "vcf";
      const path = await save({
        title: kind === "csv" ? t("导出为 CSV") : t("导出为 vCard"),
        defaultPath: t("通讯录.{0}", [extension]),
        filters: [{ name: kind === "csv" ? "CSV" : "vCard", extensions: [extension] }],
      });
      if (!path) return;
      await runAction(async () => {
        const result = await api.exportContacts(path, kind, "active");
        flash(t("已导出 {0} 条到 {1}", [result.count, result.path]));
      });
    },
    [flash, runAction],
  );

  /** 选文件并读预览（这一步不写库）。 */
  const openImport = useCallback(async () => {
    const chosen = await open({
      title: t("选择要导入的联系人文件"),
      multiple: false,
      filters: [{ name: t("联系人文件"), extensions: ["csv", "vcf"] }],
    });
    if (!chosen || Array.isArray(chosen)) return;
    try {
      const preview = await api.previewContactImport(chosen);
      setImportPath(chosen);
      setImportOverwrite(false);
      setImportPreview(preview);
    } catch (caught: unknown) {
      flash(describeError(caught));
    }
  }, [flash]);

  /** 用户在预览里换了邮箱列，重新解析一次。 */
  const pickEmailColumn = useCallback(
    async (column: number) => {
      try {
        const preview = await api.previewContactImport(importPath, column);
        setImportPreview(preview);
      } catch (caught: unknown) {
        flash(describeError(caught));
      }
    },
    [flash, importPath],
  );

  const confirmImport = useCallback(async () => {
    if (!importPreview) return;
    setImportBusy(true);
    try {
      const outcome = await api.applyContactImport(importPreview.entries, importOverwrite);
      setImportPreview(undefined);
      flash(
        t("导入完成：新增 {0}，覆盖 {1}，跳过 {2}", [outcome.imported, outcome.overwritten, outcome.skipped]),
      );
      reload();
    } catch (caught: unknown) {
      flash(describeError(caught));
    } finally {
      setImportBusy(false);
    }
  }, [flash, importOverwrite, importPreview, reload]);

  const menuTarget = menu;

  return (
    <div
      className="contacts-workspace"
      style={{
        gridTemplateColumns: `${widths.sidebar}px ${RESIZER_WIDTH}px ${widths.list}px ${RESIZER_WIDTH}px minmax(0, 1fr)`,
      }}
    >
      <aside className="contacts-groups">
        <div className="contacts-groups-head">
          <input
            type="search"
            className="contacts-search"
            placeholder={t("搜名字或邮箱")}
            aria-label={t("搜索联系人")}
            value={keyword}
            onChange={(event) => setKeyword(event.target.value)}
          />
        </div>

        <ul className="contacts-group-list">
          <li>
            <button
              type="button"
              className={view.kind === "all" ? "contacts-group active" : "contacts-group"}
              onClick={() => setView({ kind: "all" })}
            >
              <span className="contacts-group-name">{t("所有联系人")}</span>
              <span className="contacts-group-count">{counts.active}</span>
            </button>
          </li>
          {counts.ungrouped > 0 || view.kind === "ungrouped" ? (
            <li>
              <button
                type="button"
                className={view.kind === "ungrouped" ? "contacts-group active" : "contacts-group"}
                onClick={() => setView({ kind: "ungrouped" })}
              >
                <span className="contacts-group-name">{t("未分组")}</span>
                <span className="contacts-group-count">{counts.ungrouped}</span>
              </button>
            </li>
          ) : null}
          {groups.map((group) => (
            <li key={group.id}>
              <button
                type="button"
                className={
                  view.kind === "group" && view.id === group.id
                    ? "contacts-group active"
                    : "contacts-group"
                }
                title={t("右键可以重命名或删除")}
                onClick={() => setView({ kind: "group", id: group.id })}
                onContextMenu={(event) => openGroupMenu(event, group)}
              >
                <span className="contacts-group-name">{group.name}</span>
                <span className="contacts-group-count">{group.memberCount}</span>
              </button>
            </li>
          ))}
          <li>
            <button
              type="button"
              className={view.kind === "hidden" ? "contacts-group active" : "contacts-group"}
              onClick={() => setView({ kind: "hidden" })}
            >
              <span className="contacts-group-name">{t("已隐藏")}</span>
              <span className="contacts-group-count">{counts.hidden}</span>
            </button>
          </li>
        </ul>

        <div className="contacts-groups-foot">
          <button type="button" onClick={() => void createGroup()}>
            {t("新建分组")}</button>
          <button type="button" onClick={() => void openImport()}>
            {t("导入")}</button>
          <button type="button" onClick={() => setExportOpen((value) => !value)}>
            {t("导出")}</button>
          {exportOpen ? (
            <div className="contacts-export-menu">
              <button type="button" onClick={() => void exportContacts("csv")}>
                {t("CSV（Excel 能开）")}</button>
              <button type="button" onClick={() => void exportContacts("vcf")}>
                {t("vCard（手机 / Outlook 能认）")}</button>
            </div>
          ) : null}
        </div>
      </aside>

      <PaneResizer
        label={t("分组栏宽度")}
        value={widths.sidebar}
        min={paneLimits.sidebar.min}
        max={paneLimits.sidebar.max}
        onChange={setSidebar}
        onReset={resetPaneWidths}
        onCommit={persistPaneWidths}
      />

      <section className="contacts-list-pane">
        <div className="contacts-list-head">
          <span className="contacts-list-title">
            {viewTitle(view, groups)} · {visible.length}
          </span>
          {view.kind === "hidden" ? null : (
            <>
              <button type="button" className="primary" onClick={startCreate}>
                {t("新建联系人")}</button>
              <button
                type="button"
                onClick={() => {
                  setBatchMode((value) => !value);
                  setChecked(new Set());
                }}
              >
                {batchMode ? t("退出批量") : t("批量")}
              </button>
            </>
          )}
        </div>

        {batchMode ? (
          <div className="contacts-batch-bar">
            <span>{t("已选 ")}{checked.size}{t(" 位")}</span>
            <button type="button" disabled={checked.size === 0} onClick={hideChecked}>
              {t("批量隐藏")}</button>
            <select
              aria-label={t("移到分组")}
              value=""
              disabled={checked.size === 0}
              onChange={(event) => {
                const value = event.target.value;
                if (value === "") return;
                void moveChecked(value === "none" ? null : Number(value));
              }}
            >
              <option value="">{t("移到分组…")}</option>
              <option value="none">{t("未分组")}</option>
              {groups.map((group) => (
                <option key={group.id} value={String(group.id)}>
                  {group.name}
                </option>
              ))}
            </select>
          </div>
        ) : null}

        {loading ? <p className="hint">{t("正在读通讯录…")}</p> : null}
        {error ? <p className="error">{error}</p> : null}
        {!loading && !error && visible.length === 0 ? (
          <p className="hint">
            {keyword.trim() === ""
              ? t("这里还没有联系人。同步一次邮件，或者自己新建一位。")
              : t("没有匹配的联系人。")}
          </p>
        ) : null}

        <div className="contacts-list" ref={scrollRef}>
          <div
            className="contacts-list-inner"
            style={{ height: `${virtualizer.getTotalSize()}px` }}
          >
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              if (!row) return null;
              if (row.kind === "section") {
                return (
                  <div
                    key={row.key}
                    className="contacts-section"
                    style={{ transform: `translateY(${item.start}px)`, height: `${item.size}px` }}
                  >
                    {row.label}
                  </div>
                );
              }
              const contact = row.contact;
              return (
                <div
                  key={row.key}
                  className={
                    contact.id === selectedId
                      ? "contacts-row selected"
                      : "contacts-row"
                  }
                  style={{ transform: `translateY(${item.start}px)`, height: `${item.size}px` }}
                  title={t("右键可以加进分组")}
                  onContextMenu={(event) => openContactMenu(event, contact)}
                >
                  {batchMode ? (
                    <input
                      type="checkbox"
                      aria-label={t("选择 {0}", [contactLabel(contact)])}
                      checked={checked.has(contact.id)}
                      onChange={(event) => {
                        setChecked((old) => {
                          const next = new Set(old);
                          if (event.target.checked) next.add(contact.id);
                          else next.delete(contact.id);
                          return next;
                        });
                      }}
                    />
                  ) : null}
                  <button type="button" className="contacts-row-main" onClick={() => { setSelectedId(contact.id); setEditing(false); }}>
                    <span className="contacts-row-name">{contactLabel(contact)}</span>
                    <span className="contacts-row-email">{contact.email}</span>
                  </button>
                </div>
              );
            })}
          </div>
        </div>
      </section>

      <PaneResizer
        label={t("联系人列表宽度")}
        value={widths.list}
        min={paneLimits.list.min}
        max={paneLimits.list.max}
        onChange={setList}
        onReset={resetPaneWidths}
        onCommit={persistPaneWidths}
      />

      <aside className="contacts-detail" aria-label={t("联系人详情")}>
        {notice ? <p className="notice">{notice}</p> : null}

        {editing ? (
          <form
            className="contacts-form"
            onSubmit={(event) => {
              event.preventDefault();
              void saveDraft();
            }}
          >
            <h2>{selectedId === undefined ? t("新建联系人") : t("编辑联系人")}</h2>
            <label className="field">
              <span>{t("显示名")}</span>
              <input
                aria-label={t("显示名")}
                value={draft.name}
                onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              />
            </label>
            <label className="field">
              <span>{t("邮箱")}</span>
              <input
                aria-label={t("邮箱")}
                value={draft.email}
                onChange={(event) => setDraft({ ...draft, email: event.target.value })}
              />
            </label>
            <label className="field">
              <span>{t("分组")}</span>
              <select
                aria-label={t("联系人分组")}
                value={draft.groupId === null ? "" : String(draft.groupId)}
                onChange={(event) =>
                  setDraft({
                    ...draft,
                    groupId: event.target.value === "" ? null : Number(event.target.value),
                  })
                }
              >
                <option value="">{t("未分组")}</option>
                {groups.map((group) => (
                  <option key={group.id} value={String(group.id)}>
                    {group.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="field">
              <span>{t("备注")}</span>
              <textarea
                aria-label={t("备注")}
                rows={6}
                value={draft.note}
                onChange={(event) => setDraft({ ...draft, note: event.target.value })}
              />
            </label>
            <div className="form-actions">
              <button type="submit" className="primary" disabled={saving}>
                {t("保存")}</button>
              <button type="button" onClick={cancelEdit} disabled={saving}>
                {t("取消")}</button>
            </div>
          </form>
        ) : selected ? (
          <div className="contacts-card">
            <h2>{contactLabel(selected)}</h2>
            <p className="path">{selected.email}</p>
            <dl className="contacts-meta">
              <dt>{t("分组")}</dt>
              <dd>{selected.groupName ?? t("未分组")}</dd>
              <dt>{t("最近联系")}</dt>
              <dd>{lastUsedLabel(selected.lastUsedAt)}</dd>
              <dt>{t("来源")}</dt>
              <dd>{selected.source === "manual" ? t("手动添加") : t("自动收集")}</dd>
            </dl>
            <p className="contacts-note">{selected.note.trim() === "" ? t("没有备注") : selected.note}</p>
            <div className="form-actions">
              <button type="button" className="primary" onClick={() => onCompose(selected)}>
                {t("写邮件")}</button>
              <button type="button" onClick={() => void copyAddress(selected)}>
                {t("复制地址")}</button>
              {selected.hidden ? (
                <>
                  <button type="button" onClick={() => restoreContact(selected)}>
                    {t("恢复")}</button>
                  <button type="button" className="danger" onClick={() => purgeContact(selected)}>
                    {t("彻底删除")}</button>
                </>
              ) : (
                <>
                  <button type="button" onClick={() => startEdit(selected)}>
                    {t("编辑")}</button>
                  <button type="button" className="danger" onClick={() => hideContact(selected)}>
                    {t("删除")}</button>
                </>
              )}
            </div>
          </div>
        ) : (
          <p className="hint">{t("从中间选一个人看看详情。")}</p>
        )}

      </aside>

      {importPreview ? (
        <div className="modal-backdrop" role="dialog" aria-label={t("导入预览")}>
          <div className="modal contacts-import">
            <h2>{t("导入预览")}</h2>
            <p>
              {t("读到 ")}
              {importPreview.entries.length}
              {t(" 条：新增 ")}
              {importPreview.newCount}
              {t("，库里已有")}{" "}
              {importPreview.duplicateCount}
              {t("，坏行 ")}
              {importPreview.problems.length}
            </p>
            {importPreview.headers.length > 0 && importPreview.emailColumn !== null ? (
              <label className="field">
                <span>{t("哪一列是邮箱")}</span>
                <select
                  aria-label={t("邮箱列")}
                  value={String(importPreview.emailColumn)}
                  onChange={(event) => void pickEmailColumn(Number(event.target.value))}
                >
                  {importPreview.headers.map((head, index) => (
                    <option key={`${head}-${index}`} value={String(index)}>
                      {head}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            <label className="checkbox">
              <input
                type="checkbox"
                checked={importOverwrite}
                onChange={(event) => setImportOverwrite(event.target.checked)}
              />
              {t("用文件里的名字与备注覆盖已有联系人（不勾就跳过重复的）")}</label>
            {importPreview.problems.length > 0 ? (
              <ul className="contacts-problems">
                {importPreview.problems.map((problem) => (
                  <li key={problem.line}>
                    {t("第 ")}{problem.line}{t(" 行：")}{problem.reason}
                  </li>
                ))}
              </ul>
            ) : null}
            <div className="form-actions">
              <button type="button" className="primary" disabled={importBusy} onClick={() => void confirmImport()}>
                {t("确认导入")}</button>
              <button type="button" disabled={importBusy} onClick={() => setImportPreview(undefined)}>
                {t("取消")}</button>
            </div>
          </div>
        </div>
      ) : null}

      {menuTarget === undefined ? null : (
        <div
          className="contacts-menu"
          role="menu"
          aria-label={menuTarget.kind === "group" ? t("分组菜单") : t("联系人菜单")}
          style={{
            left: Math.min(menuTarget.x, Math.max(8, window.innerWidth - 220)),
            top: Math.min(menuTarget.y, Math.max(8, window.innerHeight - 200)),
          }}
          onClick={(event) => event.stopPropagation()}
        >
          {menuTarget.kind === "group" ? (
            <>
              <button
                type="button"
                role="menuitem"
                className="contacts-menu-item"
                onClick={() => {
                  const target = menuTarget.group;
                  closeMenu();
                  void renameGroup(target);
                }}
              >
                {t("重命名")}</button>
              <button
                type="button"
                role="menuitem"
                className="contacts-menu-item danger"
                onClick={() => {
                  const target = menuTarget.group;
                  closeMenu();
                  void removeGroup(target);
                }}
              >
                {t("删除分组")}</button>
            </>
          ) : (
            <div
              className="contacts-menu-item contacts-menu-parent"
              onMouseEnter={() => setSubmenuOpen(true)}
              onMouseLeave={() => setSubmenuOpen(false)}
            >
              <span>{t("添加到")}</span>
              <span aria-hidden="true">▸</span>
              {submenuOpen ? (
                <div className="contacts-submenu" role="menu" aria-label={t("选择分组")}>
                  <button
                    type="button"
                    role="menuitem"
                    className="contacts-menu-item"
                    onClick={() => {
                      const target = menuTarget.contact;
                      closeMenu();
                      void moveOneToGroup(target, null);
                    }}
                  >
                    {t("未分组")}</button>
                  {groups.map((group) => (
                    <button
                      key={group.id}
                      type="button"
                      role="menuitem"
                      className="contacts-menu-item"
                      onClick={() => {
                        const target = menuTarget.contact;
                        closeMenu();
                        void moveOneToGroup(target, group.id);
                      }}
                    >
                      {group.name}
                    </button>
                  ))}
                  {groups.length === 0 ? (
                    <span className="contacts-menu-empty">{t("还没有分组")}</span>
                  ) : null}
                </div>
              ) : null}
            </div>
          )}
        </div>
      )}
    </div>
  );
}