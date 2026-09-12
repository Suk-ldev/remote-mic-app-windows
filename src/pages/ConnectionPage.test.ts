import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AudioEndpoint,
  AudioSnapshot,
  ConnectionSnapshot,
  RuntimeSnapshot,
  ShortcutCaptureEdge,
  VoiceHotkeySettings,
} from "../lib/bridge";
import ConnectionPage from "./ConnectionPage.vue";

const emptyConnection: ConnectionSnapshot = {
  phase: "idle",
  remoteName: null,
  remoteModel: "unknown",
  capabilities: null,
  voiceState: "idle",
  decodedSamples: 0,
  generation: 0,
  reconnectAttempt: 0,
  powerNotificationsAvailable: false,
  lastError: null,
};

const emptyAudio: AudioSnapshot = {
  phase: "unconfigured",
  selectedEndpointId: null,
  selectedEndpointName: null,
  queuedSamples: 0,
  submittedSamples: 0,
  generation: 0,
  lastError: null,
};

const runtime: RuntimeSnapshot = {
  appVersion: "0.1.0",
  platform: {
    platform: "windows",
    windowsApiAvailable: true,
    bleScanAvailable: true,
    bleVoiceReady: false,
    wasapiReady: false,
    rawInputReady: false,
    sendInputReady: true,
    verificationStatus: "测试",
    connection: emptyConnection,
    audio: emptyAudio,
    rawInput: {
      phase: "stopped",
      matchedDeviceCount: 0,
      rawEventCount: 0,
      semanticEdgeCount: 0,
      lastButton: null,
      lastIsPressed: null,
      activeButtons: [],
      lastError: null,
    },
    buttonMapping: {
      enabled: true,
      gateActive: false,
      listenerActive: false,
      swallowedEdges: 0,
      leakedDowns: 0,
      firedGestures: 0,
      lastFired: null,
      lastError: null,
    },
  },
};

const cableEndpoint: AudioEndpoint = {
  id: "cable-input",
  name: "CABLE Input (VB-Audio Virtual Cable)",
  isVirtualCableCandidate: true,
};

const mocks = vi.hoisted(() => ({
  endpoints: [] as AudioEndpoint[],
  captureHandler: null as ((edge: ShortcutCaptureEdge) => void) | null,
  getConnectionSnapshot: vi.fn(),
  getAudioSnapshot: vi.fn(),
  listAudioEndpoints: vi.fn(),
  selectAudioEndpoint: vi.fn(),
  openVbCableDownloadPage: vi.fn(),
  getVoiceHoldHotkey: vi.fn(),
  setVoiceHoldHotkey: vi.fn(),
  startShortcutCapture: vi.fn(),
  stopShortcutCapture: vi.fn(),
  subscribeShortcutCaptureEdges: vi.fn(),
}));

vi.mock("../lib/bridge", async (importOriginal) => {
  const original = await importOriginal<typeof import("../lib/bridge")>();
  return {
    ...original,
    getConnectionSnapshot: mocks.getConnectionSnapshot,
    getAudioSnapshot: mocks.getAudioSnapshot,
    listAudioEndpoints: mocks.listAudioEndpoints,
    selectAudioEndpoint: mocks.selectAudioEndpoint,
    openVbCableDownloadPage: mocks.openVbCableDownloadPage,
    getVoiceHoldHotkey: mocks.getVoiceHoldHotkey,
    setVoiceHoldHotkey: mocks.setVoiceHoldHotkey,
    startShortcutCapture: mocks.startShortcutCapture,
    stopShortcutCapture: mocks.stopShortcutCapture,
    subscribeShortcutCaptureEdges: mocks.subscribeShortcutCaptureEdges,
  };
});

describe("VB-CABLE first-launch guidance", () => {
  beforeEach(() => {
    mocks.endpoints = [];
    mocks.getConnectionSnapshot.mockResolvedValue(emptyConnection);
    mocks.getAudioSnapshot.mockResolvedValue(emptyAudio);
    mocks.listAudioEndpoints.mockImplementation(async () => mocks.endpoints);
    mocks.selectAudioEndpoint.mockImplementation(async (endpointId: string) => ({
      ...emptyAudio,
      phase: "ready",
      selectedEndpointId: endpointId,
      selectedEndpointName: cableEndpoint.name,
    }));
    mocks.openVbCableDownloadPage.mockResolvedValue(undefined);
    mocks.getVoiceHoldHotkey.mockResolvedValue({
      chord: { keys: ["left_control", "left_windows"] },
      mode: "hold",
      activateWetype: true,
    });
    mocks.setVoiceHoldHotkey.mockImplementation(async (hotkey: VoiceHotkeySettings) => hotkey);
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("groups each status dot with its heading for vertical alignment", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    const headings = wrapper.findAll(".status-heading");
    expect(headings).toHaveLength(2);
    for (const heading of headings) {
      expect(heading.find(".status-dot").exists()).toBe(true);
      expect(heading.find("strong").exists()).toBe(true);
    }
    wrapper.unmount();
  });

  it("automatically selects the only VB-CABLE endpoint when no endpoint was configured", async () => {
    mocks.endpoints = [cableEndpoint];
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    expect(mocks.selectAudioEndpoint).toHaveBeenCalledOnce();
    expect(mocks.selectAudioEndpoint).toHaveBeenCalledWith(cableEndpoint.id);
    expect(wrapper.text()).toContain("已自动选择 CABLE Input");
    expect(wrapper.text()).not.toContain("需要安装 VB-CABLE");
    expect(wrapper.text()).not.toContain("系统语音输入");
    wrapper.unmount();
  });

  it("waits for the saved endpoint and does not replace an existing selection", async () => {
    const savedAudio: AudioSnapshot = {
      ...emptyAudio,
      phase: "ready",
      selectedEndpointId: "saved-speaker",
      selectedEndpointName: "已保存的扬声器",
    };
    let resolveAudio: ((snapshot: AudioSnapshot) => void) | undefined;
    mocks.endpoints = [cableEndpoint];
    mocks.getAudioSnapshot.mockImplementationOnce(
      () =>
        new Promise<AudioSnapshot>((resolve) => {
          resolveAudio = resolve;
        }),
    );

    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();
    expect(mocks.listAudioEndpoints).not.toHaveBeenCalled();

    resolveAudio?.(savedAudio);
    await flushPromises();

    expect(mocks.listAudioEndpoints).toHaveBeenCalledOnce();
    expect(mocks.selectAudioEndpoint).not.toHaveBeenCalled();
    wrapper.unmount();
  });

  it("shows the official installation action when VB-CABLE is unavailable", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    expect(mocks.selectAudioEndpoint).not.toHaveBeenCalled();
    expect(wrapper.text()).toContain("需要安装 VB-CABLE");
    expect(wrapper.text()).toContain("完成后需重启电脑");

    await wrapper.get(".vb-cable-callout .primary-button").trigger("click");
    await flushPromises();
    expect(mocks.openVbCableDownloadPage).toHaveBeenCalledOnce();
    wrapper.unmount();
  });
});

describe("语音输入快捷键设置", () => {
  const wechatHotkey: VoiceHotkeySettings = {
    chord: { keys: ["left_control", "left_windows"] },
    mode: "hold",
    activateWetype: true,
  };

  function findButton(wrapper: VueWrapper, label: string) {
    const button = wrapper.findAll("button").find((candidate) => candidate.text() === label);
    if (!button) throw new Error(`找不到按钮：${label}`);
    return button;
  }

  beforeEach(() => {
    mocks.endpoints = [cableEndpoint];
    mocks.getConnectionSnapshot.mockResolvedValue(emptyConnection);
    mocks.getAudioSnapshot.mockResolvedValue(emptyAudio);
    mocks.listAudioEndpoints.mockImplementation(async () => mocks.endpoints);
    mocks.selectAudioEndpoint.mockResolvedValue(emptyAudio);
    mocks.getVoiceHoldHotkey.mockResolvedValue(wechatHotkey);
    mocks.setVoiceHoldHotkey.mockImplementation(async (hotkey: VoiceHotkeySettings) => hotkey);
    mocks.startShortcutCapture.mockResolvedValue(undefined);
    mocks.stopShortcutCapture.mockResolvedValue(undefined);
    mocks.captureHandler = null;
    mocks.subscribeShortcutCaptureEdges.mockImplementation(
      async (handler: (edge: ShortcutCaptureEdge) => void) => {
        mocks.captureHandler = handler;
        return () => {
          mocks.captureHandler = null;
        };
      },
    );
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("shows the saved shortcut and its trigger mode", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    expect(wrapper.text()).toContain("语音输入快捷键");
    expect(wrapper.text()).toContain("左 Ctrl + 左 Win");
    expect(wrapper.text()).toContain("按住说话");
    wrapper.unmount();
  });

  it("switches to single-trigger mode without losing the configured chord", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "单次触发").trigger("click");
    await flushPromises();

    expect(mocks.setVoiceHoldHotkey).toHaveBeenCalledWith({
      chord: { keys: ["left_control", "left_windows"] },
      mode: "toggle",
      activateWetype: true,
    });
    expect(wrapper.text()).toContain("单次触发");
    wrapper.unmount();
  });

  it("applies the Windows voice preset as a single-trigger chord without IME switching", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "Windows 语音输入").trigger("click");
    await flushPromises();

    expect(mocks.setVoiceHoldHotkey).toHaveBeenCalledWith({
      chord: { keys: ["left_windows", "h"] },
      mode: "toggle",
      activateWetype: false,
    });
    wrapper.unmount();
  });

  // Typeless 等工具常用纯修饰键快捷键（右 Alt）：录入不能要求"终止键"，
  // 全部松开即提交。
  it("captures a modifier-only custom shortcut and saves it on release", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "录入自定义按键").trigger("click");
    await flushPromises();
    expect(mocks.startShortcutCapture).toHaveBeenCalledOnce();

    mocks.captureHandler?.({ key: "right_alt", isPressed: true });
    await flushPromises();
    expect(wrapper.text()).toContain("右 Alt");

    mocks.captureHandler?.({ key: "right_alt", isPressed: false });
    await flushPromises();

    expect(mocks.stopShortcutCapture).toHaveBeenCalled();
    expect(mocks.setVoiceHoldHotkey).toHaveBeenCalledWith({
      chord: { keys: ["right_alt"] },
      mode: "hold",
      activateWetype: true,
    });
    expect(wrapper.text()).toContain("语音输入快捷键已录入：右 Alt");
    wrapper.unmount();
  });

  it("keeps the whole combination pressed during capture, not just the last key", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "录入自定义按键").trigger("click");
    await flushPromises();

    mocks.captureHandler?.({ key: "left_control", isPressed: true });
    mocks.captureHandler?.({ key: "left_shift", isPressed: true });
    mocks.captureHandler?.({ key: "f5", isPressed: true });
    mocks.captureHandler?.({ key: "f5", isPressed: false });
    mocks.captureHandler?.({ key: "left_shift", isPressed: false });
    await flushPromises();
    expect(mocks.setVoiceHoldHotkey).not.toHaveBeenCalled();

    mocks.captureHandler?.({ key: "left_control", isPressed: false });
    await flushPromises();

    expect(mocks.setVoiceHoldHotkey).toHaveBeenCalledWith({
      chord: { keys: ["left_control", "left_shift", "f5"] },
      mode: "hold",
      activateWetype: true,
    });
    wrapper.unmount();
  });

  // 原生钩子之外，录入还接 window 捕获阶段的键盘事件（不依赖元素焦点）。
  it("also accepts capture edges from window keyboard events", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "录入自定义按键").trigger("click");
    await flushPromises();

    window.dispatchEvent(new KeyboardEvent("keydown", { code: "AltRight" }));
    await flushPromises();
    expect(wrapper.text()).toContain("右 Alt");

    window.dispatchEvent(new KeyboardEvent("keyup", { code: "AltRight" }));
    await flushPromises();

    expect(mocks.setVoiceHoldHotkey).toHaveBeenCalledWith({
      chord: { keys: ["right_alt"] },
      mode: "hold",
      activateWetype: true,
    });
    wrapper.unmount();
  });

  it("releases the native capture hook when the page unmounts mid-capture", async () => {
    const wrapper = mount(ConnectionPage, { props: { runtime } });
    await flushPromises();

    await findButton(wrapper, "录入自定义按键").trigger("click");
    await flushPromises();

    wrapper.unmount();
    await flushPromises();
    expect(mocks.stopShortcutCapture).toHaveBeenCalled();
    expect(mocks.captureHandler).toBeNull();
  });
});
