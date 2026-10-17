//! 通讯录三栏页回归：入口位置、分区排序、详情、增删改、分组、已隐藏。

import { open, save } from "@tauri-apps/plugin-dialog";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ContactsWorkspace, {
  buildListRows,
  contactBucket,
  sortContacts,
} from "../ContactsWorkspace";
import WorkspaceShell from "../WorkspaceShell";
import { api, type Contact, type ContactGroup } from "../api";

vi.mock("@tanstack/react-virtual", () => ({
  useVirtualizer: ({
    count,
    estimateSize,
  }: {
    count: number;
    estimateSize: (index: number) => number;
  }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: index * estimateSize(index),
        size: estimateSize(index),
      })),
    getTotalSize: () => count * 58,
  }),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn(),
  save: vi.fn(),
}));

vi.mock("../api", () => ({
  describeError: (error: unknown) => (error instanceof Error ? error.message : String(error)),
  api: {
    listContacts: vi.fn(),
    listContactGroups: vi.fn(),
    contactCounts: vi.fn(),
    createContact: vi.fn(),
    updateContact: vi.fn(),
    hideContact: vi.fn(),
    restoreContact: vi.fn(),
    purgeContact: vi.fn(),
    createContactGroup: vi.fn(),
    deleteContactGroup: vi.fn(),
    renameContactGroup: vi.fn(),
    exportContacts: vi.fn(),
    previewContactImport: vi.fn(),
    applyContactImport: vi.fn(),
  },
}));

/** 造一位联系人；只填测试关心的字段。 */
function contact(overrides: Partial<Contact> = {}): Contact {
  return {
    id: 1,
    name: "张三",
    email: "zhangsan@example.com",
    note: "",
    groupId: null,
    groupName: null,
    source: "auto",
    hidden: false,
    lastUsedAt: null,
    createdAt: "2026-10-01T00:00:00.000Z",
    updatedAt: "2026-10-01T00:00:00.000Z",
    ...overrides,
  };
}

const CONTACTS: Contact[] = [
  contact({ id: 1, name: "阿里", email: "ali@example.com" }),
  contact({ id: 2, name: "Bob", email: "bob@example.com" }),
  contact({ id: 3, name: "3M", email: "team@example.com" }),
  contact({ id: 4, name: "", email: "lisi@other.com" }),
];

const HIDDEN: Contact[] = [
  contact({ id: 9, name: "被藏起来的", email: "hidden@example.com", hidden: true }),
];

const GROUPS: ContactGroup[] = [{ id: 7, name: "客户", memberCount: 2 }];

beforeEach(() => {
  vi.mocked(api.listContacts).mockImplementation(async (_keyword, _limit, scope) =>
    scope === "hidden" ? HIDDEN : CONTACTS,
  );
  vi.mocked(api.listContactGroups).mockResolvedValue(GROUPS);
  vi.mocked(api.contactCounts).mockResolvedValue({ active: 4, hidden: 1, ungrouped: 1 });
  vi.mocked(api.createContact).mockResolvedValue(5);
  vi.mocked(api.updateContact).mockResolvedValue(undefined);
  vi.mocked(api.hideContact).mockResolvedValue(undefined);
  vi.mocked(api.restoreContact).mockResolvedValue(undefined);
  vi.mocked(api.purgeContact).mockResolvedValue(undefined);
  vi.mocked(api.createContactGroup).mockResolvedValue(8);
  vi.mocked(open).mockResolvedValue(null);
  vi.mocked(save).mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

/** 等第一屏数据落位。 */
async function waitForLoaded() {
  await waitFor(() => expect(screen.getByLabelText("搜索联系人")).toBeTruthy());
  await screen.findByText("阿里");
}

function renderShell(mode: "inbox" | "contacts" | "settings") {
  return render(
    <WorkspaceShell
      mode={mode}
      inbox={<div>收件箱占位</div>}
      contacts={<div>通讯录占位</div>}
      settings={<div>设置占位</div>}
    />,
  );
}

describe("列表分区与排序", () => {
  it("数字区排在最前，中文按拼音落到字母区", () => {
    const rows = buildListRows(sortContacts(CONTACTS));
    const labels = rows.filter((row) => row.kind === "section").map((row) => row.label);
    expect(labels[0]).toBe("0-9");
    expect(labels).toContain("A");
    expect(labels).toContain("B");
    const first = rows.find((row) => row.kind === "contact");
    expect(first?.kind === "contact" && first.contact.name).toBe("3M");
  });

  it("分不出字母的落到其他区", () => {
    expect(contactBucket(contact({ name: "★★★" }))).toBe("#");
  });
});

describe("通讯录页", () => {
  it("三栏都在，中间按分区列出联系人", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    // 第一栏
    expect(screen.getByText("所有联系人")).toBeTruthy();
    expect(screen.getByText("已隐藏")).toBeTruthy();
    expect(screen.getByText("客户")).toBeTruthy();
    // 第二栏
    expect(screen.getByText("3M")).toBeTruthy();
    expect(screen.getByText("Bob")).toBeTruthy();
    // 第三栏空态
    expect(screen.getByText("从中间选一个人看看详情。")).toBeTruthy();
  });

  it("点一行，第三栏显示详情", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.click(screen.getByText("Bob"));

    const detail = within(await screen.findByLabelText("联系人详情"));
    expect(detail.getByText("bob@example.com")).toBeTruthy();
    expect(detail.getByText("自动收集")).toBeTruthy();
    expect(screen.getByRole("button", { name: "写邮件" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "编辑" })).toBeTruthy();
  });

  it("搜索框按名字或邮箱当场过滤", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.change(screen.getByLabelText("搜索联系人"), { target: { value: "lisi" } });

    expect(screen.queryByText("阿里")).toBeNull();
    // 没显示名的人，这行的标题和地址都是邮箱，所以会出现不止一次。
    expect(screen.getAllByText("lisi@other.com").length).toBeGreaterThan(0);
  });

  it("点「写邮件」把这位联系人交给外壳", async () => {
    const onCompose = vi.fn();
    render(<ContactsWorkspace onCompose={onCompose} />);
    await waitForLoaded();

    fireEvent.click(screen.getByText("阿里"));
    fireEvent.click(await screen.findByRole("button", { name: "写邮件" }));

    expect(onCompose).toHaveBeenCalledWith(expect.objectContaining({ email: "ali@example.com" }));
  });

  it("新建联系人会带着当前分组提交", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.click(screen.getByText("客户"));
    fireEvent.click(screen.getByRole("button", { name: "新建联系人" }));
    fireEvent.change(screen.getByLabelText("显示名"), { target: { value: "新来的" } });
    fireEvent.change(screen.getByLabelText("邮箱"), { target: { value: "new@example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    await waitFor(() =>
      expect(api.createContact).toHaveBeenCalledWith({
        name: "新来的",
        email: "new@example.com",
        note: "",
        groupId: 7,
      }),
    );
  });

  it("保存失败时留在表单里并把原因说出来", async () => {
    vi.mocked(api.createContact).mockRejectedValue(new Error("邮箱格式不对"));
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.click(screen.getByRole("button", { name: "新建联系人" }));
    fireEvent.change(screen.getByLabelText("邮箱"), { target: { value: "坏的" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));

    expect(await screen.findByText("邮箱格式不对")).toBeTruthy();
    expect((screen.getByLabelText("邮箱") as HTMLInputElement).value).toBe("坏的");
  });

  it("删除是软删：确认后调用隐藏", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.click(screen.getByText("阿里"));
    fireEvent.click(await screen.findByRole("button", { name: "删除" }));

    await waitFor(() => expect(api.hideContact).toHaveBeenCalledWith(1));
  });

  it("「已隐藏」视图给出恢复与彻底删除", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.click(screen.getByText("已隐藏"));
    expect(await screen.findByText("被藏起来的")).toBeTruthy();

    fireEvent.click(screen.getByText("被藏起来的"));
    expect(await screen.findByRole("button", { name: "恢复" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "彻底删除" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "写邮件" })).toBeTruthy();
  });

  it("没建过分组时不出现自建分组按钮", async () => {
    vi.mocked(api.listContactGroups).mockResolvedValue([]);
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    expect(screen.queryByText("客户")).toBeNull();
    expect(screen.queryByRole("button", { name: "新建分组" })).toBeNull();
    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    expect(screen.getByRole("menuitem", { name: "新建分组" })).toBeTruthy();
  });

  it.each(["联系人分组栏", "所有联系人", "未分组", "已隐藏"])(
    "右键%s显示管理入口，底部没有常驻按钮",
    async (target) => {
      render(<ContactsWorkspace onCompose={() => {}} />);
      await waitForLoaded();

      expect(screen.queryByRole("button", { name: "新建分组" })).toBeNull();
      expect(screen.queryByRole("button", { name: "导入" })).toBeNull();
      expect(screen.queryByRole("button", { name: "导出" })).toBeNull();
      const element = target === "联系人分组栏"
        ? screen.getByLabelText(target)
        : screen.getByText(target, { selector: ".contacts-group-name" });
      fireEvent.contextMenu(element);
      const menu = screen.getByRole("menu", { name: "分组菜单" });
      expect(within(menu).getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
        "新建分组", "导入", "导出为 CSV", "导出为 vCard",
      ]);
    },
  );

  it("搜索框保留原生文字编辑右键菜单", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    expect(fireEvent.contextMenu(screen.getByLabelText("搜索联系人"))).toBe(true);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("菜单新建分组沿用原有流程，并关闭菜单", async () => {
    vi.spyOn(window, "prompt").mockReturnValue("朋友");
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: "新建分组" }));

    await waitFor(() => expect(api.createContactGroup).toHaveBeenCalledWith("朋友"));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("取消新建分组不写入", async () => {
    vi.spyOn(window, "prompt").mockReturnValue(null);
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: "新建分组" }));

    expect(api.createContactGroup).not.toHaveBeenCalled();
  });

  it("取消导入文件选择不读取或写入联系人", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: "导入" }));

    await waitFor(() => expect(open).toHaveBeenCalled());
    expect(api.previewContactImport).not.toHaveBeenCalled();
    expect(api.applyContactImport).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("右键导入仍需在预览中确认后才写入", async () => {
    const entries = [{ name: "朋友", email: "friend@example.com", note: "", group: "" }];
    vi.mocked(open).mockResolvedValue("D:/contacts.csv");
    vi.mocked(api.previewContactImport).mockResolvedValue({
      headers: [], emailColumn: null, entries, problems: [], duplicateCount: 0, newCount: 1,
    });
    vi.mocked(api.applyContactImport).mockResolvedValue({
      imported: 1, skipped: 0, overwritten: 0, invalid: 0, groupsCreated: 0,
    });
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: "导入" }));
    const preview = await screen.findByRole("dialog", { name: "导入预览" });
    expect(api.previewContactImport).toHaveBeenCalledWith("D:/contacts.csv");
    expect(api.applyContactImport).not.toHaveBeenCalled();

    fireEvent.click(within(preview).getByRole("button", { name: "确认导入" }));
    await waitFor(() => expect(api.applyContactImport).toHaveBeenCalledWith(entries, false));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it.each([
    ["导出为 CSV", "csv"],
    ["导出为 vCard", "vcf"],
  ] as const)("取消%s的保存对话框不导出", async (label, kind) => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: label }));
    await waitFor(() => expect(save).toHaveBeenCalledWith(expect.objectContaining({
      filters: [expect.objectContaining({ extensions: [kind] })],
    })));
    expect(api.exportContacts).not.toHaveBeenCalled();
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it.each([
    ["导出为 CSV", "csv"],
    ["导出为 vCard", "vcf"],
  ] as const)("%s仍导出全部可见联系人", async (label, kind) => {
    const path = `D:/contacts.${kind}`;
    vi.mocked(save).mockResolvedValue(path);
    vi.mocked(api.exportContacts).mockResolvedValue({ count: 4, path });
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();
    fireEvent.click(screen.getByText("已隐藏"));

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByRole("menuitem", { name: label }));

    await waitFor(() => expect(api.exportContacts).toHaveBeenCalledWith(path, kind, "active"));
  });

  it("键盘可以打开菜单，Escape 关闭并回到原位置", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();
    const sidebar = screen.getByLabelText("联系人分组栏");
    sidebar.focus();

    fireEvent.keyDown(sidebar, { key: "F10", shiftKey: true });
    expect(screen.getByRole("menuitem", { name: "新建分组" })).toBe(document.activeElement);
    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(sidebar);

    fireEvent.keyDown(screen.getByText("客户").closest("button")!, { key: "ContextMenu" });
    expect(screen.getByRole("menuitem", { name: "重命名" })).toBeTruthy();
  });

  it("点击外部或滚动会关闭菜单", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.click(screen.getByText("从中间选一个人看看详情。"));
    expect(screen.queryByRole("menu")).toBeNull();

    fireEvent.contextMenu(screen.getByLabelText("联系人分组栏"));
    fireEvent.scroll(screen.getByLabelText("联系人分组栏"));
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("右键分组给出重命名与删除", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    vi.mocked(api.deleteContactGroup).mockResolvedValue(undefined);
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByText("客户"));

    const menu = await screen.findByRole("menu", { name: "分组菜单" });
    expect(within(menu).getByRole("menuitem", { name: "重命名" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "新建分组" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "导入" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "导出为 CSV" })).toBeTruthy();
    expect(within(menu).getByRole("menuitem", { name: "导出为 vCard" })).toBeTruthy();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "删除分组" }));

    await waitFor(() => expect(api.deleteContactGroup).toHaveBeenCalledWith(7));
  });

  it("右键联系人，悬停「添加到」能选分组", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByText("阿里"));
    const menu = await screen.findByRole("menu", { name: "联系人菜单" });
    expect(within(menu).queryByRole("menuitem", { name: "导入" })).toBeNull();

    const parent = within(menu).getByText("添加到").closest(".contacts-menu-parent");
    expect(parent).toBeTruthy();
    fireEvent.mouseEnter(parent as Element);

    const submenu = await screen.findByRole("menu", { name: "选择分组" });
    fireEvent.click(within(submenu).getByRole("menuitem", { name: "客户" }));

    await waitFor(() =>
      expect(api.updateContact).toHaveBeenCalledWith(
        1,
        expect.objectContaining({ groupId: 7, email: "ali@example.com" }),
      ),
    );
  });
});

describe("三栏页面外壳", () => {
  it("当前页面可见，另外两页保持挂载但不显示", () => {
    renderShell("contacts");

    expect(screen.getByText("通讯录占位")).toBeTruthy();
    expect(screen.getByText("收件箱占位").closest("[hidden]")).toBeTruthy();
    expect(screen.getByText("设置占位").closest("[hidden]")).toBeTruthy();
    expect(document.querySelectorAll(".workspace-pane")).toHaveLength(3);
  });

  it("通讯录标题栏能返回收件箱", async () => {
    const onBackToInbox = vi.fn();
    render(
      <ContactsWorkspace
        onCompose={() => {}}
        onBackToInbox={onBackToInbox}
      />,
    );
    await waitForLoaded();

    fireEvent.click(screen.getByRole("button", { name: "返回收件箱" }));

    expect(onBackToInbox).toHaveBeenCalledOnce();
  });
});
