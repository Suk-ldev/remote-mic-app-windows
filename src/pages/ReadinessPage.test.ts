import { describe, expect, it, vi } from "vitest";
import { flushPromises, mount } from "@vue/test-utils";
import ReadinessPage from "./ReadinessPage.vue";
import type { RuntimeSnapshot } from "../lib/bridge";

const mocks = vi.hoisted(() => ({
  getConnectionSnapshot: vi.fn(),
  getAudioSnapshot: vi.fn(),
  listAudioEndpoints: vi.fn(),
  getVoiceHoldHotkey: vi.fn(),
  getButtonMappings: vi.fn(),
  openVbCableDownloadPage: vi.fn(),
}));

vi.mock("../lib/bridge", async (importOriginal) => {
  const original = await importOriginal<typeof import("../lib/bridge")>();
  return { ...original, ...mocks };
});

function setup(overrides: Partial<Record<string, unknown>> = {}) {
  mocks.getConnectionSnapshot.mockResolvedValue({
    phase: "idle",
    remoteName: null,
    remoteModel: "unknown",
    batteryLevel: null,
    capabilities: null,
    voiceState: "idle",
    decodedSamples: 0,
    generation: 0,
    reconnectAttempt: 0,
    powerNotificationsAvailable: false,
    lastError: null,
    ...(overrides.connection as object),
  });
  mocks.getAudioSnapshot.mockResolvedValue({
    phase: "idle",
    selectedEndpointId: null,
    selectedEndpointName: null,
    queuedSamples: 0,
    submittedSamples: 0,
    generation: 0,
    lastError: null,
    ...(overrides.audio as object),
  });
  mocks.listAudioEndpoints.mockResolvedValue(overrides.endpoints ?? []);
  mocks.getVoiceHoldHotkey.mockResolvedValue(
    overrides.hotkey ?? { chord: null, mode: "hold", activateWetype: false },
  );
  mocks.getButtonMappings.mockResolvedValue(
    overrides.mappings ?? { enabled: true, actions: {} },
  );
  mocks.openVbCableDownloadPage.mockResolvedValue(undefined);
  return mount(ReadinessPage, { props: { runtime: null as RuntimeSnapshot | null } });
}

describe("准备页", () => {
  it("未就绪时列出剩余必需项，并标出必需与可选", async () => {
    const wrapper = setup();
    await flushPromises();

    expect(wrapper.text()).toContain("还有 4 项必需的没完成");
    const rows = wrapper.findAll(".readiness-row");
    expect(rows).toHaveLength(5);
    expect(rows[0]!.text()).toContain("必需");
    expect(rows[4]!.text()).toContain("可选");
    wrapper.unmount();
  });

  it("全部就绪时给出可以开始用的结论", async () => {
    const wrapper = setup({
      connection: { phase: "ready", remoteName: "小米遥控器 2 Pro" },
      audio: { selectedEndpointName: "CABLE Input" },
      endpoints: [
        { id: "cable", name: "CABLE Input", isVirtualCableCandidate: true, isDefault: false },
      ],
      hotkey: { chord: { keys: ["left_control"] }, mode: "hold", activateWetype: false },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("必需项都已就绪");
    expect(wrapper.text()).toContain("小米遥控器 2 Pro");
    wrapper.unmount();
  });

  it("未就绪项带错误码，就绪项不带", async () => {
    const wrapper = setup();
    await flushPromises();

    const rows = wrapper.findAll(".readiness-row");
    expect(rows[0]!.text()).toContain("READY-REMOTE-DISCONNECTED");
    expect(rows[1]!.text()).toContain("READY-CABLE-MISSING");
    // 按键映射是可选项，不给错误码。
    expect(rows[4]!.text()).not.toContain("错误码");
    wrapper.unmount();
  });

  it("读到电量时显示在遥控器一行，读不到不占位", async () => {
    const ready = {
      connection: { phase: "ready", remoteName: "小米遥控器 2 Pro", batteryLevel: 78 },
      audio: { selectedEndpointName: "CABLE Input" },
      endpoints: [
        { id: "cable", name: "CABLE Input", isVirtualCableCandidate: true, isDefault: false },
      ],
      hotkey: { chord: { keys: ["left_control"] }, mode: "hold", activateWetype: false },
    };
    const wrapper = setup(ready);
    await flushPromises();
    expect(wrapper.findAll(".readiness-row")[0]!.text()).toContain("电量 78%");
    wrapper.unmount();

    const withoutBattery = setup({ ...ready, connection: { phase: "ready", remoteName: "小米遥控器 2 Pro" } });
    await flushPromises();
    const row = withoutBattery.findAll(".readiness-row")[0]!;
    expect(row.text()).toContain("小米遥控器 2 Pro");
    expect(row.text()).not.toContain("电量");
    withoutBattery.unmount();
  });

  it("点操作按钮时把目标页面冒泡给外壳", async () => {
    const wrapper = setup();
    await flushPromises();

    const goConnect = wrapper
      .findAll(".readiness-row button")
      .find((button) => button.text().trim() === "去连接")!;
    await goConnect.trigger("click");
    expect(wrapper.emitted("navigate")?.[0]).toEqual(["connection"]);
    wrapper.unmount();
  });
});
