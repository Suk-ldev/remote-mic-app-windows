<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  getAudioSnapshot,
  getButtonMappings,
  getConnectionSnapshot,
  getVoiceHoldHotkey,
  listAudioEndpoints,
  openVbCableDownloadPage,
  type AudioSnapshot,
  type ButtonMappings,
  type ConnectionSnapshot,
  type RuntimeSnapshot,
  type VoiceHotkeySettings,
} from "../lib/bridge";
import type { PageId } from "../navigation";

defineProps<{ runtime: RuntimeSnapshot | null }>();
const emit = defineEmits<{ navigate: [PageId] }>();

type Tone = "success" | "warning" | "pending";

interface ReadinessItem {
  id: string;
  title: string;
  required: boolean;
  tone: Tone;
  status: string;
  detail: string;
  /**
   * 未就绪时显示的错误码：报障时报一个码比截图整页有用得多。
   * 就绪时为 null，不占版面。
   */
  code: string | null;
  action?: { label: string; run: () => void | Promise<void> };
}

const connection = ref<ConnectionSnapshot | null>(null);
const audio = ref<AudioSnapshot | null>(null);
const hasVirtualCable = ref<boolean | null>(null);
const voiceHotkey = ref<VoiceHotkeySettings | null>(null);
const mappings = ref<ButtonMappings | null>(null);
const loadError = ref("");
let pollTimer: ReturnType<typeof setInterval> | undefined;

async function refresh() {
  try {
    const [nextConnection, nextAudio, endpoints, hotkey, nextMappings] = await Promise.all([
      getConnectionSnapshot(),
      getAudioSnapshot(),
      listAudioEndpoints(),
      getVoiceHoldHotkey(),
      getButtonMappings(),
    ]);
    connection.value = nextConnection;
    audio.value = nextAudio;
    hasVirtualCable.value = endpoints.some((endpoint) => endpoint.isVirtualCableCandidate);
    voiceHotkey.value = hotkey;
    mappings.value = nextMappings;
    loadError.value = "";
  } catch (error) {
    loadError.value = error instanceof Error ? error.message : String(error);
  }
}

const items = computed<ReadinessItem[]>(() => {
  const connected =
    connection.value?.phase === "ready" || connection.value?.phase === "streaming";
  const cable = hasVirtualCable.value;
  const endpointChosen = Boolean(audio.value?.selectedEndpointName);
  const hotkeyKeys = voiceHotkey.value?.chord?.keys.length ?? 0;
  const mapped = Object.values(mappings.value?.actions ?? {}).some((actions) =>
    [actions.single, actions.double, actions.long].some((sequence) => sequence.length > 0),
  );

  return [
    {
      id: "remote",
      title: "遥控器已连接",
      required: true,
      tone: connected ? "success" : "warning",
      status: connected
        ? [connection.value?.remoteName ?? "已连接", batteryText.value]
            .filter(Boolean)
            .join(" · ")
        : "未连接",
      detail: "先在 Windows 蓝牙设置里配对遥控器，再在连接页选中它。",
      code: connected ? null : "READY-REMOTE-DISCONNECTED",
      action: { label: "去连接", run: () => emit("navigate", "connection") },
    },
    {
      id: "cable",
      title: "虚拟声卡 VB-CABLE",
      required: true,
      tone: cable === null ? "pending" : cable ? "success" : "warning",
      status: cable === null ? "读取中" : cable ? "已安装" : "未安装",
      detail: "遥控器麦克风要经虚拟声卡才能被输入法听到。装完需要重启一次 Windows。",
      code: cable === false ? "READY-CABLE-MISSING" : null,
      action: { label: "打开官方下载页", run: () => openVbCableDownloadPage() },
    },
    {
      id: "endpoint",
      title: "语音输出设备",
      required: true,
      tone: endpointChosen ? "success" : "warning",
      status: audio.value?.selectedEndpointName ?? "未选择",
      detail: "选 CABLE Input；再在语音工具里把麦克风设为 CABLE Output。",
      code: endpointChosen ? null : "READY-ENDPOINT-UNSET",
      action: { label: "去选择", run: () => emit("navigate", "connection") },
    },
    {
      id: "hotkey",
      title: "语音工具快捷键",
      required: true,
      tone: hotkeyKeys > 0 ? "success" : "warning",
      status: hotkeyKeys > 0 ? "已配置" : "未配置",
      detail: "语音键按下时注入这个快捷键，语音工具才会开始录音。搜狗和豆包可在连接页一键读取它们自己的设置。",
      code: hotkeyKeys > 0 ? null : "READY-VOICE-HOTKEY-UNSET",
      action: { label: "去设置", run: () => emit("navigate", "connection") },
    },
    {
      id: "mapping",
      title: "按键映射",
      required: false,
      tone: mapped ? "success" : "pending",
      status: mapped ? "已配置" : "未配置",
      detail: "可选。不配置时遥控器按键保持原始行为；映射页有一键套用的预设方案。",
      code: null,
      action: { label: "去配置", run: () => emit("navigate", "buttons") },
    },
  ];
});

/** 电量文案：读不到就不显示，不用"未知"占位。 */
const batteryText = computed(() => {
  const level = connection.value?.batteryLevel;
  return typeof level === "number" ? `电量 ${level}%` : "";
});

const remaining = computed(
  () => items.value.filter((item) => item.required && item.tone !== "success").length,
);

onMounted(() => {
  void refresh();
  pollTimer = setInterval(() => {
    void refresh();
  }, 2_000);
});

onUnmounted(() => {
  if (pollTimer) clearInterval(pollTimer);
});
</script>

<template>
  <section>
    <header class="page-header">
      <div>
        <h1>准备</h1>
        <p class="muted">
          {{ remaining === 0 ? "必需项都已就绪，可以开始用了。" : `还有 ${remaining} 项必需的没完成，从上往下点一遍即可。` }}
        </p>
      </div>
    </header>

    <div v-if="loadError" class="error-banner">{{ loadError }}</div>

    <article class="card readiness-list">
      <div v-for="item in items" :key="item.id" class="readiness-row">
        <span class="badge" :class="item.tone">{{ item.status }}</span>
        <div class="readiness-body">
          <strong>
            {{ item.title }}
            <small class="muted">{{ item.required ? "必需" : "可选" }}</small>
          </strong>
          <p class="muted">{{ item.detail }}</p>
          <p v-if="item.code" class="muted readiness-code">错误码 {{ item.code }}（报障时带上这个码）</p>
        </div>
        <button
          v-if="item.action"
          class="secondary-button"
          type="button"
          @click="item.action.run()"
        >
          {{ item.action.label }}
        </button>
      </div>
    </article>
  </section>
</template>
