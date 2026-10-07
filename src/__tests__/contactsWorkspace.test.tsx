//! 通讯录三栏页回归：入口位置、分区排序、详情、增删改、分组、已隐藏。

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

function renderShell(mode: "inbox" | "contacts" | "settings", onModeChange = () => {}) {
  return render(
    <WorkspaceShell
      mode={mode}
      onModeChange={onModeChange}
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
    expect(screen.getByRole("button", { name: "新建分组" })).toBeTruthy();
  });

  it("右键分组给出重命名与删除", async () => {
    vi.spyOn(window, "confirm").mockReturnValue(true);
    vi.mocked(api.deleteContactGroup).mockResolvedValue(undefined);
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByText("客户"));

    const menu = await screen.findByRole("menu", { name: "分组菜单" });
    expect(within(menu).getByRole("menuitem", { name: "重命名" })).toBeTruthy();
    fireEvent.click(within(menu).getByRole("menuitem", { name: "删除分组" }));

    await waitFor(() => expect(api.deleteContactGroup).toHaveBeenCalledWith(7));
  });

  it("右键联系人，悬停「添加到」能选分组", async () => {
    render(<ContactsWorkspace onCompose={() => {}} />);
    await waitForLoaded();

    fireEvent.contextMenu(screen.getByText("阿里"));
    const menu = await screen.findByRole("menu", { name: "联系人菜单" });

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

describe("最左设置栏", () => {
  it("三个入口按「收件箱 / 通讯录 / 设置」排列", () => {
    renderShell("contacts");

    const labels = screen
      .getAllByRole("button")
      .map((node) => node.getAttribute("aria-label"));
    expect(labels).toEqual(["收件箱", "通讯录", "设置"]);
    expect(screen.getByRole("button", { name: "通讯录" }).getAttribute("aria-current")).toBe(
      "page",
    );
  });

  it("点通讯录会把新入口交给外壳", () => {
    const onModeChange = vi.fn();
    renderShell("inbox", onModeChange);

    fireEvent.click(screen.getByRole("button", { name: "通讯录" }));

    expect(onModeChange).toHaveBeenCalledWith("contacts");
  });
});
