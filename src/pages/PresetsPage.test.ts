import { beforeEach, describe, expect, it, vi } from "vitest";
import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import PresetsPage from "./PresetsPage.vue";
import type { RuntimeSnapshot } from "../lib/bridge";

vi.mock("../lib/bridge", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/bridge")>();
  return {
    ...actual,
    applyMappingPreset: vi.fn(async () => ({ enabled: true, actions: {} })),
    resetButtonMappings: vi.fn(async () => ({ enabled: true, actions: {} })),
    exportButtonMappingConfiguration: vi.fn(async () => true),
    importButtonMappingConfiguration: vi.fn(async () => ({ enabled: true, actions: {} })),
    getInjectionHoldMs: vi.fn(async () => 30),
    setInjectionHoldMs: vi.fn(async (millis: number) => millis),
    getAppProfiles: vi.fn(async () => ({ enabled: false, bindings: {} })),
    saveAppProfiles: vi.fn(async (bindings: unknown) => bindings),
    getActiveAppProfile: vi.fn(async () => null),
  };
});

import {
  applyMappingPreset,
  exportButtonMappingConfiguration,
  importButtonMappingConfiguration,
  resetButtonMappings,
  saveAppProfiles,
  setInjectionHoldMs,
} from "../lib/bridge";

async function mountPage(): Promise<VueWrapper> {
  const wrapper = mount(PresetsPage, { props: { runtime: null as RuntimeSnapshot | null } });
  await flushPromises();
  return wrapper;
}

beforeEach(() => {
  vi.mocked(applyMappingPreset).mockClear();
  vi.mocked(resetButtonMappings).mockClear();
  vi.mocked(exportButtonMappingConfiguration).mockClear();
  vi.mocked(importButtonMappingConfiguration).mockClear();
  vi.mocked(setInjectionHoldMs).mockClear();
  vi.mocked(saveAppProfiles).mockClear();
});

describe("方案页", () => {
  it("预设方案：选择后套用整套映射", async () => {
    const wrapper = await mountPage();
    const select = wrapper.find("#mapping-preset");
    expect(select.findAll("option").length).toBeGreaterThan(1);
    await select.setValue("reading");

    const applyButton = wrapper
      .findAll(".primary-button")
      .find((button) => button.text().trim() === "套用")!;
    await applyButton.trigger("click");
    await flushPromises();

    expect(applyMappingPreset).toHaveBeenCalledWith("reading");
    expect(wrapper.text()).toContain("已套用");
  });

  it("按应用切换方案：开关打开后可添加与删除绑定", async () => {
    const wrapper = await mountPage();
    expect(wrapper.find(".app-profile-panel").exists()).toBe(false);

    await wrapper.find("#app-profile-toggle").setValue(true);
    await flushPromises();
    expect(saveAppProfiles).toHaveBeenCalledWith({ enabled: true, bindings: {} });

    await wrapper.find(".app-profile-panel .text-action-input").setValue("chrome");
    await wrapper
      .findAll(".app-profile-panel .chip")
      .find((chip) => chip.text().trim() === "添加绑定")!
      .trigger("click");
    await flushPromises();
    expect(saveAppProfiles).toHaveBeenLastCalledWith({
      enabled: true,
      bindings: { chrome: "generic" },
    });
    expect(wrapper.find(".app-profile-row").text()).toContain("chrome");

    await wrapper.find(".app-profile-row .sequence-remove").trigger("click");
    await flushPromises();
    expect(saveAppProfiles).toHaveBeenLastCalledWith({ enabled: true, bindings: {} });
  });

  it("按键保持时长：改动即保存并提示", async () => {
    const wrapper = await mountPage();
    await wrapper.find("#injection-hold").setValue(50);
    await wrapper.find("#injection-hold").trigger("change");
    await flushPromises();
    expect(setInjectionHoldMs).toHaveBeenCalledWith(50);
    expect(wrapper.text()).toContain("按键保持时长已设为 50 毫秒");
  });

  it("配置备份：导入、导出与恢复默认都在这一页", async () => {
    const wrapper = await mountPage();
    const button = (label: string) =>
      wrapper.findAll("button").find((item) => item.text() === label)!;

    await button("导出配置…").trigger("click");
    await flushPromises();
    expect(exportButtonMappingConfiguration).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("按键映射配置已导出");

    await button("导入配置…").trigger("click");
    await flushPromises();
    expect(importButtonMappingConfiguration).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("按键映射配置已导入并生效");

    await button("恢复默认").trigger("click");
    await flushPromises();
    expect(resetButtonMappings).toHaveBeenCalledOnce();
    expect(wrapper.text()).toContain("已恢复默认");
  });

  it("提供回到按键页的入口", async () => {
    const wrapper = await mountPage();
    await wrapper
      .findAll("button")
      .find((item) => item.text() === "去配按键")!
      .trigger("click");
    expect(wrapper.emitted("navigate")?.[0]).toEqual(["buttons"]);
  });
});
