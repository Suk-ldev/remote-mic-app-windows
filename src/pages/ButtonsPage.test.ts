import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import ButtonsPage from "./ButtonsPage.vue";

type EdgeHandler = (edge: { button: string; isPressed: boolean }) => void;
type GestureHandler = (gesture: { button: string; trigger: string }) => void;
type ShortcutCaptureHandler = (edge: { key: string; isPressed: boolean }) => void;

let edgeHandler: EdgeHandler | null = null;
let gestureHandler: GestureHandler | null = null;
let shortcutCaptureHandler: ShortcutCaptureHandler | null = null;

vi.mock("../lib/bridge", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/bridge")>();
  return {
    ...actual,
    getButtonMappings: vi.fn(async () => ({
      enabled: true,
      actions: {
        ok: {
          single: [{ type: "shortcut", chord: { keys: ["enter"] } }],
          double: [],
          long: [],
        },
      },
    })),
    getButtonMappingSnapshot: vi.fn(async () => ({
      enabled: true,
      gateActive: true,
      listenerActive: true,
      swallowedEdges: 3,
      leakedDowns: 0,
      firedGestures: 1,
      lastFired: null,
      lastError: null,
    })),
    saveButtonMappings: vi.fn(async (mappings: unknown) => mappings),
    testButtonMapping: vi.fn(async () => ({
      available: true,
      submittedBatches: 1,
      submittedEvents: 2,
      lastError: null,
    })),
    subscribeButtonEdges: vi.fn(async (handler: EdgeHandler) => {
      edgeHandler = handler;
      return () => {};
    }),
    subscribeButtonGestures: vi.fn(async (handler: GestureHandler) => {
      gestureHandler = handler;
      return () => {};
    }),
    startShortcutCapture: vi.fn(async () => undefined),
    stopShortcutCapture: vi.fn(async () => undefined),
    subscribeShortcutCaptureEdges: vi.fn(async (handler: ShortcutCaptureHandler) => {
      shortcutCaptureHandler = handler;
      return () => {};
    }),
  };
});

import {
  getButtonMappings,
  subscribeButtonEdges,
  subscribeButtonGestures,
  saveButtonMappings,
  startShortcutCapture,
  stopShortcutCapture,
} from "../lib/bridge";
import type { ButtonMappings, RuntimeSnapshot } from "../lib/bridge";

const runtime: RuntimeSnapshot = {
  appVersion: "0.1.0",
  platform: {
    platform: "windows",
    windowsApiAvailable: true,
    bleScanAvailable: true,
    bleVoiceReady: true,
    wasapiReady: false,
    rawInputReady: true,
    sendInputReady: true,
    verificationStatus: "测试",
    connection: {
      phase: "ready",
      remoteName: "小米蓝牙语音遥控器",
      remoteModel: "rc003",
      batteryLevel: null,
      capabilities: null,
      voiceState: "idle",
      decodedSamples: 0,
      generation: 0,
      reconnectAttempt: 0,
      powerNotificationsAvailable: false,
      lastError: null,
    },
    audio: {
      phase: "unsupported",
      selectedEndpointId: null,
      selectedEndpointName: null,
      queuedSamples: 0,
      submittedSamples: 0,
      generation: 0,
      lastError: null,
    },
    rawInput: {
      phase: "ready",
      profileId: null,
      matchedDeviceCount: 1,
      rawEventCount: 0,
      semanticEdgeCount: 0,
      lastButton: null,
      lastIsPressed: null,
      activeButtons: [],
      lastError: null,
    },
    buttonMapping: {
      enabled: true,
      gateActive: true,
      listenerActive: true,
      swallowedEdges: 3,
      leakedDowns: 0,
      firedGestures: 1,
      lastFired: null,
      lastError: null,
    },
  },
};

async function mountPage(model: "rc001" | "rc003" | "unknown" = "rc003"): Promise<VueWrapper> {
  const snapshot =
    model === "rc003"
      ? runtime
      : {
          ...runtime,
          platform: {
            ...runtime.platform,
            connection: { ...runtime.platform.connection, remoteModel: model },
          },
        };
  const wrapper = mount(ButtonsPage, { props: { runtime: snapshot } });
  await vi.waitFor(() => {
    if (!edgeHandler || !gestureHandler) throw new Error("事件订阅未完成");
  });
  return wrapper;
}

beforeEach(() => {
  edgeHandler = null;
  gestureHandler = null;
  shortcutCaptureHandler = null;
  vi.mocked(getButtonMappings).mockClear();
  vi.mocked(subscribeButtonEdges).mockClear();
  vi.mocked(subscribeButtonGestures).mockClear();
  vi.mocked(saveButtonMappings).mockClear();
  vi.mocked(startShortcutCapture).mockClear();
  vi.mocked(stopShortcutCapture).mockClear();
});

describe("buttons mapping page", () => {
  it("renders the 12 canvas buttons, the voice card and 36 trigger cells", async () => {
    const wrapper = await mountPage();
    // 小米遥控器实物只有这 12 个可映射键：画布 12 张卡 + 语音卡，没有画布外按键，
    // 因此"其他按键"整块不出现（静音键只存在于 Google TV 遥控器）。
    expect(wrapper.findAll(".mapping-card")).toHaveLength(13);
    expect(wrapper.findAll(".extra-card")).toHaveLength(0);
    expect(wrapper.find(".extra-buttons").exists()).toBe(false);
    expect(wrapper.findAll(".mapping-cell")).toHaveLength(36);
    const voiceCard = wrapper.find(".voice-card");
    expect(voiceCard.text()).toContain("语音键");
    expect(voiceCard.text()).toContain("按住说话");
  });

  it("Google TV 机型：画布外补出 YouTube / Netflix 两个键", async () => {
    const wrapper = await mountPage();
    await wrapper.setProps({
      runtime: {
        ...runtime,
        platform: {
          ...runtime.platform,
          rawInput: { ...runtime.platform.rawInput, profileId: "google_tv" },
        },
      },
    });
    await flushPromises();
    const extras = wrapper.find(".extra-buttons").text();
    expect(extras).toContain("YouTube");
    expect(extras).toContain("Netflix");
    // 静音键是 Google TV 遥控器独有的（小米遥控器没有），示意图上也没有位置。
    expect(extras).toContain("静音");
    // Google 遥控器没有菜单键：画布卡片仍是小米示意图，这里只补差集。
    expect(extras).not.toContain("菜单");
  });

  it("does not register listeners or polling after unmounting during initial load", async () => {
    let resolveMappings!: (value: Awaited<ReturnType<typeof getButtonMappings>>) => void;
    const pendingMappings = new Promise<Awaited<ReturnType<typeof getButtonMappings>>>(
      (resolve) => {
        resolveMappings = resolve;
      },
    );
    vi.mocked(getButtonMappings).mockImplementationOnce(() => pendingMappings);
    const intervalSpy = vi.spyOn(window, "setInterval");

    const wrapper = mount(ButtonsPage, { props: { runtime } });
    await flushPromises();
    wrapper.unmount();
    resolveMappings({ enabled: true, actions: {} });
    await flushPromises();

    expect(subscribeButtonEdges).not.toHaveBeenCalled();
    expect(subscribeButtonGestures).not.toHaveBeenCalled();
    expect(intervalSpy).not.toHaveBeenCalled();
    intervalSpy.mockRestore();
  });

  it("immediately releases a listener that resolves after the page is unmounted", async () => {
    let resolveUnlisten!: (unlisten: () => void) => void;
    const pendingUnlisten = new Promise<() => void>((resolve) => {
      resolveUnlisten = resolve;
    });
    vi.mocked(subscribeButtonEdges).mockImplementationOnce(() => pendingUnlisten);
    const stopEdges = vi.fn();

    const wrapper = mount(ButtonsPage, { props: { runtime } });
    await vi.waitFor(() => expect(subscribeButtonEdges).toHaveBeenCalledOnce());
    const intervalSpy = vi.spyOn(window, "setInterval");
    wrapper.unmount();
    resolveUnlisten(stopEdges);
    await flushPromises();

    expect(stopEdges).toHaveBeenCalledOnce();
    expect(subscribeButtonGestures).not.toHaveBeenCalled();
    expect(intervalSpy).not.toHaveBeenCalled();
    intervalSpy.mockRestore();
  });

  it("saves the mapping configuration from the footer", async () => {
    const wrapper = await mountPage();
    const saveButton = wrapper
      .findAll(".mapping-footer button")
      .find((item) => item.text() === "保存配置")!;

    await saveButton.trigger("click");
    await vi.waitFor(() => expect(saveButtonMappings).toHaveBeenCalled());
    expect(wrapper.text()).toContain("配置已保存并生效");

    // 导入/导出/恢复默认已经搬到「方案」页，按键页不再重复提供。
    const footerLabels = wrapper.findAll(".mapping-footer button").map((item) => item.text());
    expect(footerLabels).not.toContain("导入配置…");
    expect(footerLabels).not.toContain("导出配置…");
    expect(footerLabels).not.toContain("恢复默认");
  });

  it("头部提供去「方案」页的入口", async () => {
    const wrapper = await mountPage();
    const link = wrapper
      .findAll(".mapping-header-controls button")
      .find((item) => item.text() === "方案设置…")!;
    await link.trigger("click");
    expect(wrapper.emitted("navigate")?.[0]).toEqual(["presets"]);
  });

  it("marks configured cells and opens the editor with the correct target", async () => {
    const wrapper = await mountPage();
    const okCard = wrapper
      .findAll(".mapping-card")
      .find((card) => card.text().includes("确定"));
    expect(okCard).toBeDefined();
    expect(okCard!.text()).toContain("Enter");

    const singleCell = okCard!.findAll(".mapping-cell")[0]!;
    expect(singleCell.classes()).toContain("set");
    await singleCell.trigger("click");
    expect(wrapper.find(".mapping-editor").text()).toContain("确定 · 单击");
  });

  it("applies a preset to the editing target and auto-persists (对齐 Mac 即时保存)", async () => {
    const wrapper = await mountPage();
    const powerCard = wrapper
      .findAll(".mapping-card")
      .find((card) => card.text().includes("电源"));
    await powerCard!.findAll(".mapping-cell")[2]!.trigger("click");

    const editor = wrapper.find(".mapping-editor");
    expect(editor.text()).toContain("电源 · 长按");
    // 点击 Esc 预设即自动保存（无需保存按钮）。
    const chips = editor.findAll(".chip");
    const escapeChip = chips.find((chip) => chip.text() === "Esc");
    await escapeChip!.trigger("click");
    await vi.waitFor(() => {
      if (vi.mocked(saveButtonMappings).mock.calls.length === 0) {
        throw new Error("自动保存未触发");
      }
    });
    const saved = vi.mocked(saveButtonMappings).mock.calls[0]![0] as ButtonMappings;
    expect(saved.actions.power!.long).toEqual([
      { type: "shortcut", chord: { keys: ["escape"] } },
    ]);

    // 禁用按键按钮：禁用当前格并自动保存。
    const disableButton = wrapper
      .findAll("button")
      .find((button) => button.text() === "禁用按键");
    expect(disableButton).toBeDefined();
    await disableButton!.trigger("click");
    await vi.waitFor(() => {
      if (vi.mocked(saveButtonMappings).mock.calls.length < 2) {
        throw new Error("禁用后未自动保存");
      }
    });
    const disabledSaved = vi.mocked(saveButtonMappings).mock.calls[1]![0] as ButtonMappings;
    // 禁用 = 空序列（未配置），不是一条"disabled"动作。
    expect(disabledSaved.actions.power!.long).toEqual([]);
  });

  it("records a physical Win+L chord directly by default", async () => {
    const wrapper = await mountPage();
    const powerCard = wrapper
      .findAll(".mapping-card")
      .find((card) => card.text().includes("电源"))!;
    await powerCard.findAll(".mapping-cell")[0]!.trigger("click");
    const captureButton = wrapper
      .findAll(".mapping-editor .chip")
      .find((button) => button.text().includes("录入自定义快捷键"))!;
    await captureButton.trigger("click");

    shortcutCaptureHandler!({ key: "left_windows", isPressed: true });
    shortcutCaptureHandler!({ key: "l", isPressed: true });
    shortcutCaptureHandler!({ key: "l", isPressed: false });
    await flushPromises();
    expect(stopShortcutCapture).not.toHaveBeenCalled();
    shortcutCaptureHandler!({ key: "left_windows", isPressed: false });
    await vi.waitFor(() => expect(stopShortcutCapture).toHaveBeenCalledOnce());
    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)?.[0] as ButtonMappings;
    expect(saved.actions.power?.single).toEqual([
      {
        type: "shortcut",
        chord: { keys: ["left_windows", "l"] },
      },
    ]);
  });

  it("records Win+L safely after the user enables fallback mode", async () => {
    const wrapper = await mountPage();
    const powerCard = wrapper
      .findAll(".mapping-card")
      .find((card) => card.text().includes("电源"))!;
    await powerCard.findAll(".mapping-cell")[0]!.trigger("click");
    const safeToggle = wrapper.find(".safe-capture-toggle input");
    const shortcutRow = wrapper.find(".custom-shortcut-row");
    const toggleRow = wrapper.find(".safe-capture-toggle");
    expect(shortcutRow.element.nextElementSibling).toBe(toggleRow.element);
    expect(safeToggle.classes()).toContain("toggle-input");
    expect(safeToggle.element.nextElementSibling?.textContent).toContain(
      "直接录入无法完成或会触发系统动作时再开启",
    );
    expect((safeToggle.element as HTMLInputElement).checked).toBe(false);
    await safeToggle.setValue(true);
    const captureButton = wrapper
      .findAll(".mapping-editor .chip")
      .find((button) => button.text().includes("录入自定义快捷键"))!;
    await captureButton.trigger("click");
    await vi.waitFor(() => expect(startShortcutCapture).toHaveBeenCalledOnce());

    const leftWin = wrapper
      .findAll(".capture-modifiers .chip")
      .find((button) => button.text() === "左 Win")!;
    await leftWin.trigger("click");
    shortcutCaptureHandler!({ key: "l", isPressed: true });
    await vi.waitFor(() => {
      const calls = vi.mocked(saveButtonMappings).mock.calls;
      const saved = calls.at(-1)?.[0] as ButtonMappings | undefined;
      const action = saved?.actions.power?.single?.[0];
      if (action?.type !== "shortcut" || action.chord.keys.join("+") !== "left_windows+l") {
        throw new Error("Win+L 未保存");
      }
    });
    await vi.waitFor(() =>
      expect(wrapper.text()).toContain("已录入 左 Win + L，松开全部按键后完成"),
    );
    shortcutCaptureHandler!({ key: "l", isPressed: false });
    await vi.waitFor(() => expect(stopShortcutCapture).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(wrapper.text()).toContain("快捷键已录入：左 Win + L"));
  });

  it("highlights the card for a pressed physical button and clears it on release", async () => {
    const wrapper = await mountPage();
    const upCard = () =>
      wrapper.findAll(".mapping-card").find((card) => card.text().includes("上"));
    expect(upCard()!.classes()).not.toContain("active");

    edgeHandler!({ button: "up", isPressed: true });
    await vi.waitFor(() => {
      if (!upCard()!.classes().includes("active")) throw new Error("未高亮");
    });
    edgeHandler!({ button: "up", isPressed: false });
    await vi.waitFor(() => {
      if (upCard()!.classes().includes("active")) throw new Error("未解除高亮");
    });
  });

  it("keeps the selection locked while pressing the remote unless unlocked", async () => {
    const wrapper = await mountPage();
    // 默认锁定：按下"返回"不改变当前选中（未选中任何键时仍为空）。
    edgeHandler!({ button: "back", isPressed: true });
    const backCard = () =>
      wrapper.findAll(".mapping-card").find((card) => card.text().includes("返回"));
    await vi.waitFor(() => {
      if (!backCard()!.classes().includes("active")) throw new Error("未高亮");
    });
    expect(backCard()!.classes()).not.toContain("selected");

    // 解锁后：按下即选中该键的编辑。
    const toggles = wrapper.findAll(".toggle-row");
    const lockToggle = toggles.find((row) => row.text().includes("锁定当前按键"));
    const input = lockToggle!.find("input");
    await input.setValue(false);
    edgeHandler!({ button: "back", isPressed: true });
    await vi.waitFor(() => {
      if (!backCard()!.classes().includes("selected")) throw new Error("未跟随选中");
    });
  });

  it("shows the fired gesture feedback from engine events", async () => {
    const wrapper = await mountPage();
    gestureHandler!({ button: "ok", trigger: "single" });
    await vi.waitFor(() => {
      // 手势反馈 = 对应格子出现闪烁态（flashed），600ms 后自动消失。
      if (!wrapper.find(".mapping-cell.flashed").exists()) {
        throw new Error("手势触发后格子未出现闪烁反馈");
      }
    });
  });

  /** 编辑器内按标签找 chip 并返回其禁用态。 */
  function chipState(wrapper: VueWrapper, label: string): boolean {
    const chip = wrapper
      .findAll(".mapping-editor .chip")
      .find((element) => element.text().includes(label));
    expect(chip, `未找到 chip：${label}`).toBeDefined();
    return (chip!.element as HTMLButtonElement).disabled;
  }

  async function openCell(
    wrapper: VueWrapper,
    cardLabel: string,
    triggerIndex: number,
  ): Promise<void> {
    const card = wrapper.findAll(".mapping-card").find((c) => c.text().includes(cardLabel));
    expect(card, `未找到卡片：${cardLabel}`).toBeDefined();
    await card!.findAll(".mapping-cell")[triggerIndex]!.trigger("click");
    expect(wrapper.find(".mapping-editor").exists()).toBe(true);
  }

  it("全开放：确定·单击所有操作可配（注入链路已真机验证）+ 单响应提示", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "确定", 0);
    expect(chipState(wrapper, "Enter")).toBe(false);
    expect(chipState(wrapper, "Home")).toBe(false);
    expect(chipState(wrapper, "空格")).toBe(false);
    expect(chipState(wrapper, "粘贴")).toBe(false);
    expect(chipState(wrapper, "录入自定义快捷键")).toBe(false);
    expect(chipState(wrapper, "＋ 添加应用")).toBe(false);
    // 武装族按键显示冷首按原生副作用提示（信息性，不门控）。
    expect(wrapper.find(".mapping-editor").text()).toContain("原生按键动作");
  });

  it("全开放：确定·双击与 TV 所有操作可配 + 各自的单响应提示", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "确定", 1);
    expect(chipState(wrapper, "Enter")).toBe(false);
    expect(chipState(wrapper, "录入自定义快捷键")).toBe(false);
    expect(chipState(wrapper, "＋ 添加应用")).toBe(false);
    expect(wrapper.find(".mapping-editor").text()).toContain("原生按键动作");

    await openCell(wrapper, "TV", 0);
    expect(chipState(wrapper, "Enter")).toBe(false);
    expect(chipState(wrapper, "静音")).toBe(false);
    expect(chipState(wrapper, "录入自定义快捷键")).toBe(false);
    expect(chipState(wrapper, "＋ 添加应用")).toBe(false);
    expect(wrapper.find(".mapping-editor").text()).toContain("遥控器优先");
  });

  it("左键与其余方向键同样开放自定义并显示结构性泄漏提示", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "左", 0);
    expect(chipState(wrapper, "←")).toBe(false);
    expect(chipState(wrapper, "退格")).toBe(false);
    expect(chipState(wrapper, "录入自定义快捷键")).toBe(false);
    expect(wrapper.find(".mapping-editor").text()).toContain("原生按键动作");

    // 与型号无关：RC001 上左键同样开放。
    const rc001 = await mountPage("rc001");
    const leftCellRc001 = rc001
      .findAll(".mapping-card")
      .find((c) => c.text().includes("左"))!
      .findAll(".mapping-cell")[0]!;
    expect((leftCellRc001.element as HTMLButtonElement).disabled).toBe(false);
  });

  it("电源（直接归因族）全开放且无单响应提示", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "电源", 2);
    expect(chipState(wrapper, "Esc")).toBe(false);
    expect(chipState(wrapper, "截图")).toBe(false);
    expect(chipState(wrapper, "录入自定义快捷键")).toBe(false);
    expect(chipState(wrapper, "＋ 添加应用")).toBe(false);
    expect(wrapper.find(".mapping-editor").text()).not.toContain("原生按键动作");
  });

  it("返回/音量±全型号开放自定义（2026-09-13：RC003 钩子投递、RC001 直接归因）", async () => {
    for (const model of ["rc003", "rc001", "unknown"] as const) {
      const wrapper = await mountPage(model);
      const backCell = wrapper
        .findAll(".mapping-card")
        .find((c) => c.text().includes("返回"))!
        .findAll(".mapping-cell")[0]!;
      expect(
        (backCell.element as HTMLButtonElement).disabled,
        `${model} 返回格子应开放`,
      ).toBe(false);
      const volumeCell = wrapper
        .findAll(".mapping-card")
        .find((c) => c.text().includes("音量"))!
        .findAll(".mapping-cell")[0]!;
      expect(
        (volumeCell.element as HTMLButtonElement).disabled,
        `${model} 音量格子应开放`,
      ).toBe(false);
      await backCell.trigger("click");
      expect(wrapper.find(".mapping-editor").exists()).toBe(true);
    }
  });

  it("原生按键（透传）：有原生键的按键可配并即时保存，无原生键的不提供该选项", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "上", 0);
    expect(chipState(wrapper, "原生按键（透传）")).toBe(false);

    const nativeChip = wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().includes("原生按键（透传）"));
    await nativeChip!.trigger("click");
    await vi.waitFor(() => {
      if (vi.mocked(saveButtonMappings).mock.calls.length === 0) {
        throw new Error("自动保存未触发");
      }
    });
    const saved = vi.mocked(saveButtonMappings).mock.calls[0]![0] as ButtonMappings;
    expect(saved.actions.up!.single).toEqual([{ type: "native" }]);
    expect(nativeChip!.classes()).toContain("selected");

    // TV 没有原生键（native_key 返回 None）：编辑器不提供该 chip。
    await openCell(wrapper, "TV", 0);
    expect(
      wrapper
        .findAll(".mapping-editor .chip")
        .some((chip) => chip.text().includes("原生按键（透传）")),
    ).toBe(false);
  });

  it("鼠标动作：点选滚轮 chip 即时保存为 mouse 动作", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "上", 0);
    const wheelChip = wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "滚轮下")!;
    await wheelChip.trigger("click");
    await vi.waitFor(() => expect(saveButtonMappings).toHaveBeenCalled());
    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(saved.actions.up?.single).toEqual([{ type: "mouse", kind: "wheel_down" }]);
    expect(wheelChip.classes()).toContain("selected");
  });

  it("文本输出：输入后点应用即时保存为 text 动作", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "上", 0);
    await wrapper.find(".text-action-input").setValue("收到");
    const applyButton = wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "应用文本")!;
    await applyButton.trigger("click");
    await vi.waitFor(() => expect(saveButtonMappings).toHaveBeenCalled());
    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(saved.actions.up?.single).toEqual([{ type: "text", value: "收到" }]);
  });

  it("动作序列：追加模式下依次加步，删除按钮去掉指定步", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "上", 0);

    // 追加模式关闭时，点动作是整格替换。
    const enterChip = wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "Enter")!;
    await enterChip.trigger("click");
    await vi.waitFor(() => expect(saveButtonMappings).toHaveBeenCalled());
    expect(
      (vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings).actions.up!.single,
    ).toHaveLength(1);

    // 开启追加后再点一个动作，序列变成两步。
    const appendToggle = wrapper.find(".sequence-append-toggle input");
    await appendToggle.setValue(true);
    const escapeChip = wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "Esc")!;
    await escapeChip.trigger("click");
    await flushPromises();
    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(saved.actions.up!.single).toEqual([
      { type: "shortcut", chord: { keys: ["enter"] } },
      { type: "shortcut", chord: { keys: ["escape"] } },
    ]);
    expect(wrapper.findAll(".sequence-step")).toHaveLength(2);

    // 删除第一步后只剩 Esc。
    await wrapper.findAll(".sequence-remove")[0]!.trigger("click");
    await flushPromises();
    const afterRemove = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(afterRemove.actions.up!.single).toEqual([
      { type: "shortcut", chord: { keys: ["escape"] } },
    ]);
  });

  it("动作序列：可以插入等待步骤，摘要用箭头串起来", async () => {
    const wrapper = await mountPage();
    await openCell(wrapper, "上", 0);
    await wrapper.find(".text-action-input").setValue("收到");
    await wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "应用文本")!
      .trigger("click");
    await flushPromises();

    await wrapper.find(".sequence-append-toggle input").setValue(true);
    await wrapper.find(".delay-input").setValue(30);
    await wrapper
      .findAll(".mapping-editor .chip")
      .find((chip) => chip.text().trim() === "添加等待步骤")!
      .trigger("click");
    await flushPromises();

    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(saved.actions.up!.single).toEqual([
      { type: "text", value: "收到" },
      { type: "delay", ms: 30 },
    ]);
    const upCard = wrapper.findAll(".mapping-card").find((card) => card.text().includes("上"))!;
    expect(upCard.text()).toContain("→");
    expect(upCard.text()).toContain("等 30ms");
  });

  it("按住不放：仅在已配快捷键的单击列出现，切换后写入 hold_shortcut", async () => {
    const wrapper = await mountPage();
    // 确定·单击在 mock 里已配 Enter 快捷键：开关应出现。
    await openCell(wrapper, "确定", 0);
    const holdToggle = wrapper
      .findAll(".safe-capture-toggle")
      .find((row) => row.text().includes("按住不放"))!;
    expect((holdToggle.find("input").element as HTMLInputElement).checked).toBe(false);
    await holdToggle.find("input").trigger("change");
    await vi.waitFor(() => expect(saveButtonMappings).toHaveBeenCalled());
    const saved = vi.mocked(saveButtonMappings).mock.calls.at(-1)![0] as ButtonMappings;
    expect(saved.actions.ok?.single).toEqual([
      {
        type: "hold_shortcut",
        chord: { keys: ["enter"] },
      },
    ]);

    // 双击列不提供按住（Rust 侧归一化会降级为点按）。
    await openCell(wrapper, "确定", 1);
    expect(
      wrapper.findAll(".safe-capture-toggle").some((row) => row.text().includes("按住不放")),
    ).toBe(false);
  });
});
