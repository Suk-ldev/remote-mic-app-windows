<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import type {
  AudioEndpoint,
  AudioSnapshot,
  ConnectionSnapshot,
  KeyCode,
  PairedRemote,
  RuntimeSnapshot,
  ShortcutCaptureEdge,
  VoiceHotkeyMode,
  VoiceHotkeySettings,
} from "../lib/bridge";
import {
  audioPhaseLabel,
  connectRemote,
  connectionPhaseLabel,
  disabledVoiceHotkey,
  disconnectRemote,
  domCodeToKeyCode,
  getAudioSnapshot,
  getConnectionSnapshot,
  getVoiceHoldHotkey,
  isModifierKeyCode,
  listAudioEndpoints,
  openVbCableDownloadPage,
  remoteModelLabel,
  scanPairedRemotes,
  selectAudioEndpoint,
  setVoiceHoldHotkey,
  startShortcutCapture,
  stopShortcutCapture,
  subscribeShortcutCaptureEdges,
  voiceHoldHotkeyLabel,
  voiceHotkeyModeLabel,
} from "../lib/bridge";

const props = defineProps<{ runtime: RuntimeSnapshot | null }>();

const emptyConnection = (): ConnectionSnapshot => ({
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
});

const emptyAudio = (): AudioSnapshot => ({
  phase: "unsupported",
  selectedEndpointId: null,
  selectedEndpointName: null,
  queuedSamples: 0,
  submittedSamples: 0,
  generation: 0,
  lastError: null,
});

const connection = ref<ConnectionSnapshot>(emptyConnection());
const audio = ref<AudioSnapshot>(emptyAudio());
const scanning = ref(false);
const connectingDeviceId = ref("");
const disconnecting = ref(false);
const devices = ref<PairedRemote[]>([]);
const scanMessage = ref("尚未扫描");
const operationMessage = ref("");
const audioEndpoints = ref<AudioEndpoint[]>([]);
const showEndpointList = ref(false);
const scanningAudio = ref(false);
const audioScanComplete = ref(false);
const selectingEndpointId = ref("");
const openingVbCablePage = ref(false);
const audioMessage = ref("尚未读取语音设备");
const voiceHotkey = ref<VoiceHotkeySettings>(disabledVoiceHotkey());
const savingVoiceHotkey = ref(false);
const voiceHotkeyMessage = ref("尚未读取快捷键设置");
let pollTimer: ReturnType<typeof setInterval> | undefined;

/**
 * 语音工具预设。`mode` 是注入形态（hold = 按住说话，toggle = 单次触发），
 * `activateWetype` 只对微信输入法开启——对 Typeless 这类独立应用强切输入法
 * 会改变用户正在使用的输入法。其他语音工具用"录入快捷键"按自身设置配。
 */
interface VoiceHotkeyPreset {
  id: string;
  label: string;
  hint: string;
  settings: VoiceHotkeySettings;
}

const voiceHotkeyPresets: VoiceHotkeyPreset[] = [
  {
    id: "wetype",
    label: "微信输入法",
    hint: "左 Ctrl + 左 Win 按住说话（微信输入法默认语音热键）",
    settings: {
      chord: { keys: ["left_control", "left_windows"] },
      mode: "hold",
      activateWetype: true,
    },
  },
  {
    id: "windows-voice",
    label: "Windows 语音输入",
    hint: "Win + H 单次触发（系统自带语音输入）",
    settings: { chord: { keys: ["left_windows", "h"] }, mode: "toggle", activateWetype: false },
  },
  {
    id: "off",
    label: "关闭",
    hint: "语音键只把声音送进虚拟声卡，不注入任何快捷键",
    settings: disabledVoiceHotkey(),
  },
];

const voiceHotkeyKeysLabel = computed(() => voiceHoldHotkeyLabel(voiceHotkey.value.chord));

const voiceHotkeyEnabled = computed(() => (voiceHotkey.value.chord?.keys.length ?? 0) > 0);

function sameKeys(left: VoiceHotkeySettings, right: VoiceHotkeySettings): boolean {
  const a = [...(left.chord?.keys ?? [])].sort().join("+");
  const b = [...(right.chord?.keys ?? [])].sort().join("+");
  return a === b;
}

function presetIsActive(preset: VoiceHotkeyPreset): boolean {
  return (
    sameKeys(preset.settings, voiceHotkey.value) &&
    preset.settings.mode === voiceHotkey.value.mode &&
    preset.settings.activateWetype === voiceHotkey.value.activateWetype
  );
}

async function saveVoiceHotkey(settings: VoiceHotkeySettings, successMessage?: string) {
  savingVoiceHotkey.value = true;
  voiceHotkeyMessage.value = "";
  try {
    voiceHotkey.value = await setVoiceHoldHotkey(settings);
    voiceHotkeyMessage.value =
      successMessage ??
      (voiceHotkey.value.chord
        ? `语音输入快捷键已设为 ${voiceHoldHotkeyLabel(voiceHotkey.value.chord)}（${voiceHotkeyModeLabel(
            voiceHotkey.value.mode,
          )}）`
        : "语音输入快捷键已关闭，语音键仅输出语音");
  } catch (error) {
    voiceHotkeyMessage.value = error instanceof Error ? error.message : String(error);
    await refreshVoiceHotkey();
  } finally {
    savingVoiceHotkey.value = false;
  }
}

async function applyVoiceHotkeyPreset(preset: VoiceHotkeyPreset) {
  await saveVoiceHotkey({
    ...preset.settings,
    chord: preset.settings.chord ? { keys: [...preset.settings.chord.keys] } : null,
  });
}

/** 只改注入形态，保留当前快捷键与输入法选项。 */
async function applyVoiceHotkeyMode(mode: VoiceHotkeyMode) {
  if (voiceHotkey.value.mode === mode) return;
  if (!voiceHotkey.value.chord) {
    voiceHotkeyMessage.value = "请先设置一个快捷键，再选择触发方式";
    return;
  }
  await saveVoiceHotkey({ ...voiceHotkey.value, mode });
}

async function applyActivateWetype(activateWetype: boolean) {
  if (!voiceHotkey.value.chord) return;
  await saveVoiceHotkey({ ...voiceHotkey.value, activateWetype });
}

async function refreshVoiceHotkey() {
  try {
    voiceHotkey.value = await getVoiceHoldHotkey();
  } catch (error) {
    voiceHotkeyMessage.value = error instanceof Error ? error.message : String(error);
  }
}

// ---------------------------------------------------------------------------
// 自定义快捷键录入
//
// 与按键页的映射录入刻意不同：语音工具的快捷键经常是纯修饰键组合
// （微信输入法的左 Ctrl + 左 Win、Typeless 常用的右 Alt 等），因此这里
// 不要求"终止键"，而是记录本次按下的最大同时按键集合，全部松开即提交。
// 录入期间由原生低级钩子接管（startShortcutCapture），Win+L 之类的系统
// 组合在到达 Shell 之前被成对吞下，不会真的锁屏。
// ---------------------------------------------------------------------------
const CAPTURE_TIMEOUT_MS = 15_000;
const MAX_CAPTURE_KEYS = 4;
const capturingHotkey = ref(false);
const captureStarting = ref(false);
const captureDisplay = ref<KeyCode[]>([]);
const capturePressed = new Set<KeyCode>();
let captureCandidate: KeyCode[] = [];
let unlistenCapture: (() => void) | null = null;
let captureTimer: ReturnType<typeof setTimeout> | null = null;
let captureRequestId = 0;
let unmounted = false;

async function beginHotkeyCapture(): Promise<void> {
  if (capturingHotkey.value || captureStarting.value) return;
  const requestId = ++captureRequestId;
  captureStarting.value = true;
  voiceHotkeyMessage.value = "";
  try {
    await startShortcutCapture();
    if (unmounted || requestId !== captureRequestId) {
      await stopShortcutCapture().catch(() => undefined);
      return;
    }
    capturePressed.clear();
    captureCandidate = [];
    captureDisplay.value = [];
    capturingHotkey.value = true;
    unlistenCapture = await subscribeShortcutCaptureEdges(handleNativeCaptureEdge);
    if (captureTimer !== null) clearTimeout(captureTimer);
    captureTimer = setTimeout(() => {
      void finishHotkeyCapture("录入已超时，请重新录入");
    }, CAPTURE_TIMEOUT_MS);
  } catch (error) {
    voiceHotkeyMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    if (requestId === captureRequestId) captureStarting.value = false;
  }
}

async function finishHotkeyCapture(message?: string): Promise<void> {
  captureRequestId += 1;
  captureStarting.value = false;
  capturingHotkey.value = false;
  if (captureTimer !== null) clearTimeout(captureTimer);
  captureTimer = null;
  unlistenCapture?.();
  unlistenCapture = null;
  await stopShortcutCapture().catch(() => undefined);
  capturePressed.clear();
  captureCandidate = [];
  captureDisplay.value = [];
  if (message) voiceHotkeyMessage.value = message;
}

function handleNativeCaptureEdge(edge: ShortcutCaptureEdge): void {
  acceptCaptureEdge(edge.key, edge.isPressed);
}

function acceptCaptureEdge(code: KeyCode, isPressed: boolean): void {
  if (!capturingHotkey.value) return;
  if (isPressed) {
    if (capturePressed.has(code)) return;
    // Esc 单独按下 = 取消；与其他键一起按下时按普通键处理。
    if (code === "escape" && capturePressed.size === 0 && captureCandidate.length === 0) {
      void finishHotkeyCapture("已取消录入");
      return;
    }
    capturePressed.add(code);
    // 记录本次按住过程中的最大组合：修饰键在前、主键在后，便于阅读。
    const pressed = [...capturePressed];
    const combination = [
      ...pressed.filter((key) => isModifierKeyCode(key)),
      ...pressed.filter((key) => !isModifierKeyCode(key)),
    ];
    if (combination.length > captureCandidate.length) captureCandidate = combination;
    captureDisplay.value = [...captureCandidate];
    return;
  }
  capturePressed.delete(code);
  if (capturePressed.size > 0 || captureCandidate.length === 0) return;
  // 全部松开 = 本次录入结束；提交时才落盘，中途松开部分键不会误提交。
  const truncated = captureCandidate.length > MAX_CAPTURE_KEYS;
  const keys = captureCandidate.slice(0, MAX_CAPTURE_KEYS);
  const label = voiceHoldHotkeyLabel({ keys });
  const current = voiceHotkey.value;
  void finishHotkeyCapture().then(() =>
    saveVoiceHotkey(
      { ...current, chord: { keys } },
      truncated
        ? `快捷键最多 ${MAX_CAPTURE_KEYS} 个键，已录入前 ${MAX_CAPTURE_KEYS} 个：${label}`
        : `语音输入快捷键已录入：${label}`,
    ),
  );
}

function handleCaptureKeydown(event: KeyboardEvent): void {
  if (!capturingHotkey.value) return;
  event.preventDefault();
  event.stopPropagation();
  const code = domCodeToKeyCode(event.code);
  if (code === null || event.repeat) return;
  acceptCaptureEdge(code, true);
}

function handleCaptureKeyup(event: KeyboardEvent): void {
  if (!capturingHotkey.value) return;
  event.preventDefault();
  event.stopPropagation();
  const code = domCodeToKeyCode(event.code);
  if (code === null) return;
  acceptCaptureEdge(code, false);
}

function handleCaptureBlur(): void {
  if (capturingHotkey.value || captureStarting.value) {
    void finishHotkeyCapture("窗口失去焦点，已取消录入");
  }
}

watch(
  () => props.runtime?.platform.connection,
  (snapshot) => {
    if (snapshot) connection.value = snapshot;
  },
  { immediate: true },
);

watch(
  () => props.runtime?.platform.audio,
  (snapshot) => {
    if (snapshot) audio.value = snapshot;
  },
  { immediate: true },
);

const connectionActive = computed(() =>
  [
    "connecting",
    "discovering",
    "awaiting_capabilities",
    "ready",
    "streaming",
    "draining",
    "reconnecting",
    "suspended",
  ].includes(connection.value.phase),
);

const atvvReady = computed(() =>
  ["ready", "streaming", "draining"].includes(connection.value.phase),
);

const audioBusy = computed(() => ["streaming", "draining"].includes(audio.value.phase));

const wasapiReady = computed(() =>
  ["ready", "streaming", "draining"].includes(audio.value.phase),
);

const virtualCableEndpoints = computed(() =>
  audioEndpoints.value.filter((endpoint) => endpoint.isVirtualCableCandidate),
);

const virtualCableInstalled = computed(() => virtualCableEndpoints.value.length > 0);

const phaseTone = computed(() => {
  if (connection.value.phase === "failed") return "error";
  if (connection.value.phase === "streaming") return "active";
  if (connection.value.phase === "ready") return "success";
  if (connectionActive.value) return "warning";
  return "pending";
});

const phaseDetail = computed(() => {
  if (connection.value.lastError) return connection.value.lastError;
  if (connection.value.capabilities) return "语音功能已确认，可以按住遥控器语音键说话";
  return "连接后即可使用遥控器语音键";
});

const audioTone = computed(() => {
  if (audio.value.phase === "failed") return "error";
  if (audio.value.phase === "streaming") return "active";
  if (audio.value.phase === "ready") return "success";
  if (audio.value.phase === "draining") return "warning";
  return "pending";
});

const audioDetail = computed(() => {
  if (audio.value.lastError) return audio.value.lastError;
  if (audio.value.selectedEndpointName) return "语音会写入选中的设备";
  return "不会自动改动系统默认设备，需要在这里明确选择";
});

async function refreshConnection() {
  try {
    connection.value = await getConnectionSnapshot();
  } catch (error) {
    operationMessage.value = error instanceof Error ? error.message : String(error);
  }
}

async function refreshAudio() {
  try {
    audio.value = await getAudioSnapshot();
    return true;
  } catch (error) {
    audioMessage.value = error instanceof Error ? error.message : String(error);
    return false;
  }
}

async function scan() {
  scanning.value = true;
  operationMessage.value = "";
  scanMessage.value = "正在寻找小米遥控器…";
  try {
    devices.value = await scanPairedRemotes();
    scanMessage.value = devices.value.length
      ? `找到 ${devices.value.length} 个已配对的小米遥控器`
      : "没有找到已配对的小米遥控器";
  } catch (error) {
    devices.value = [];
    scanMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    scanning.value = false;
  }
}

async function connect(device: PairedRemote) {
  connectingDeviceId.value = device.id;
  operationMessage.value = "";
  try {
    connection.value = await connectRemote(device.id);
    operationMessage.value = "已连接，正在确认语音功能";
  } catch (error) {
    operationMessage.value = error instanceof Error ? error.message : String(error);
    await refreshConnection();
  } finally {
    connectingDeviceId.value = "";
  }
}

async function disconnect() {
  disconnecting.value = true;
  operationMessage.value = "";
  try {
    connection.value = await disconnectRemote();
    operationMessage.value = "遥控器连接已释放，本次运行已停止自动重连";
  } catch (error) {
    operationMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    disconnecting.value = false;
  }
}

async function detectAudioEndpoints(autoSelectVirtualCable: boolean) {
  scanningAudio.value = true;
  audioMessage.value = "正在读取语音设备…";
  try {
    audioEndpoints.value = await listAudioEndpoints();
    audioScanComplete.value = true;
    const virtualCables = audioEndpoints.value.filter(
      (endpoint) => endpoint.isVirtualCableCandidate,
    );
    if (virtualCables.length === 1 && autoSelectVirtualCable && !audio.value.selectedEndpointId) {
      await chooseAudioEndpoint(virtualCables[0], true);
      return;
    }
    audioMessage.value = virtualCables.length
      ? `已检测到 ${virtualCables.length} 个 VB-CABLE 语音设备`
      : "未检测到 VB-CABLE；安装完成后需要重启电脑，再重新检测";
  } catch (error) {
    audioEndpoints.value = [];
    audioScanComplete.value = true;
    audioMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    scanningAudio.value = false;
  }
}

async function scanAudio() {
  await detectAudioEndpoints(false);
  // 用户主动读取端点 = 想看列表；选好即收起（每次只用一个端点）。
  showEndpointList.value = audioEndpoints.value.length > 0;
}

async function chooseAudioEndpoint(endpoint: AudioEndpoint, automatic = false) {
  selectingEndpointId.value = endpoint.id;
  audioMessage.value = "正在打开语音设备…";
  try {
    audio.value = await selectAudioEndpoint(endpoint.id);
    audioMessage.value = automatic
      ? `已自动选择 ${endpoint.name}`
      : `已选择 ${endpoint.name}`;
    showEndpointList.value = false;
  } catch (error) {
    audioMessage.value = error instanceof Error ? error.message : String(error);
    await refreshAudio();
  } finally {
    selectingEndpointId.value = "";
  }
}

async function openVbCablePage() {
  openingVbCablePage.value = true;
  try {
    await openVbCableDownloadPage();
    audioMessage.value = "已打开 VB-CABLE 官方下载页面；安装时需要管理员权限，完成后请重启电脑";
  } catch (error) {
    audioMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    openingVbCablePage.value = false;
  }
}

async function initializeAudio() {
  const restoredAudio = await refreshAudio();
  await detectAudioEndpoints(restoredAudio);
}

onMounted(() => {
  // 与按键页同款：录入期间的键盘事件在 window 捕获阶段接收（不依赖某个
  // 元素获得焦点），处理函数在未录入时直接返回，不影响页面其他输入。
  window.addEventListener("keydown", handleCaptureKeydown, true);
  window.addEventListener("keyup", handleCaptureKeyup, true);
  window.addEventListener("blur", handleCaptureBlur);
  void refreshConnection();
  void initializeAudio();
  void refreshVoiceHotkey();
  pollTimer = setInterval(() => {
    void refreshConnection();
    void refreshAudio();
  }, 1_000);
});

onUnmounted(() => {
  unmounted = true;
  window.removeEventListener("keydown", handleCaptureKeydown, true);
  window.removeEventListener("keyup", handleCaptureKeyup, true);
  window.removeEventListener("blur", handleCaptureBlur);
  if (pollTimer) clearInterval(pollTimer);
  // 录入中卸载页面必须解除原生钩子的接管，否则键盘保持被吞状态。
  if (capturingHotkey.value || captureStarting.value) void finishHotkeyCapture();
});
</script>

<template>
  <section>
    <header class="page-header">
      <div>
        <h1>连接与语音</h1>
      </div>
      <span class="badge" :class="phaseTone">{{ connectionPhaseLabel(connection.phase) }}</span>
    </header>

    <div class="two-column">
      <article class="card">
        <div class="card-title-row">
          <div>
            <h2>遥控器连接</h2>
            <p class="muted">连接已配对的小米遥控器。</p>
          </div>
          <button
            class="primary-button"
            type="button"
            :disabled="scanning || connectionActive || !runtime?.platform.bleScanAvailable"
            @click="scan"
          >
            {{ scanning ? "扫描中…" : "扫描已配对设备" }}
          </button>
        </div>

        <div class="status-panel" aria-live="polite">
          <div class="status-copy">
            <div class="status-heading">
              <span class="status-dot" :class="phaseTone"></span>
              <strong>{{ connection.remoteName ?? connectionPhaseLabel(connection.phase) }}</strong>
            </div>
            <small>{{ phaseDetail }}</small>
          </div>
          <button
            v-if="connectionActive"
            class="secondary-button status-action"
            type="button"
            :disabled="disconnecting"
            @click="disconnect"
          >
            {{ disconnecting ? "断开中…" : "断开" }}
          </button>
        </div>

        <p class="muted scan-summary">{{ scanMessage }}</p>
        <p v-if="operationMessage" class="operation-message">{{ operationMessage }}</p>

        <ul v-if="devices.length" class="device-list">
          <li v-for="device in devices" :key="device.id">
            <div><strong>{{ device.name }}</strong><small>{{ remoteModelLabel(device.model) }}</small></div>
            <button
              type="button"
              :disabled="connectionActive || Boolean(connectingDeviceId)"
              @click="connect(device)"
            >
              {{ connectingDeviceId === device.id ? "连接中…" : "连接" }}
            </button>
          </li>
        </ul>

        <div class="setting-list compact two-col">
          <div class="setting-row">
            <strong>设备型号</strong>
            <span>{{ remoteModelLabel(connection.remoteModel) }}</span>
          </div>
          <div class="setting-row">
            <strong>语音按键</strong>
            <span>{{ atvvReady ? "已就绪" : "正在确认" }}</span>
          </div>
          <div class="setting-row">
            <strong>睡眠唤醒自动重连</strong>
            <span>{{ connection.powerNotificationsAvailable ? "已启用" : "暂不可用" }}</span>
          </div>
          <div class="setting-row">
            <strong>语音输入快捷键</strong>
            <span>{{ voiceHotkeyKeysLabel }}</span>
          </div>
          <div class="setting-row">
            <strong>触发方式</strong>
            <span>{{ voiceHotkeyEnabled ? voiceHotkeyModeLabel(voiceHotkey.mode) : "未启用" }}</span>
          </div>
        </div>
        <p class="muted voice-hotkey-row">按住遥控器语音键说话，松开即停止；语音会送入右侧选中的设备，由微信输入法、Typeless 等工具转成文字。语音工具自己的快捷键在这里配置：先选预设或录入自定义按键，再按该工具的要求选触发方式。</p>
        <div class="button-row voice-hotkey-presets">
          <button
            v-for="preset in voiceHotkeyPresets"
            :key="preset.id"
            :class="presetIsActive(preset) ? 'primary-button' : 'secondary-button'"
            type="button"
            :title="preset.hint"
            :disabled="savingVoiceHotkey || capturingHotkey || !runtime?.platform.windowsApiAvailable || presetIsActive(preset)"
            @click="applyVoiceHotkeyPreset(preset)"
          >
            {{ preset.label }}
          </button>
          <button
            class="secondary-button"
            type="button"
            title="按下你在语音工具里设置的快捷键（可以是纯修饰键组合，如右 Alt），松开即录入"
            :disabled="savingVoiceHotkey || !runtime?.platform.windowsApiAvailable"
            @click="capturingHotkey ? finishHotkeyCapture('已取消录入') : beginHotkeyCapture()"
          >
            {{ capturingHotkey ? "录入中…（按 Esc 取消）" : "录入自定义按键" }}
          </button>
          <span v-if="capturingHotkey" class="capture-display">
            {{ captureDisplay.length ? voiceHoldHotkeyLabel({ keys: captureDisplay }) : "请按下快捷键，松开即录入" }}
          </span>
        </div>
        <div class="voice-hotkey-modes" role="group" aria-label="触发方式">
          <button
            :class="voiceHotkey.mode === 'hold' ? 'primary-button' : 'secondary-button'"
            type="button"
            title="语音开始按下快捷键、结束松开：微信输入法、Win+H 等按住即录音的工具"
            :disabled="savingVoiceHotkey || capturingHotkey || !voiceHotkeyEnabled || voiceHotkey.mode === 'hold'"
            @click="applyVoiceHotkeyMode('hold')"
          >
            按住说话
          </button>
          <button
            :class="voiceHotkey.mode === 'toggle' ? 'primary-button' : 'secondary-button'"
            type="button"
            title="语音开始点按一次、结束再点按一次：Typeless 等按一次开始、再按一次结束的工具"
            :disabled="savingVoiceHotkey || capturingHotkey || !voiceHotkeyEnabled || voiceHotkey.mode === 'toggle'"
            @click="applyVoiceHotkeyMode('toggle')"
          >
            单次触发
          </button>
        </div>
        <label class="toggle-row voice-hotkey-ime">
          <input
            type="checkbox"
            :checked="voiceHotkey.activateWetype"
            :disabled="savingVoiceHotkey || capturingHotkey || !voiceHotkeyEnabled"
            @change="applyActivateWetype(($event.target as HTMLInputElement).checked)"
          />
          <span>触发前自动切到微信输入法（只有微信输入法需要；Typeless 等独立应用请保持关闭）</span>
        </label>
        <p class="muted scan-summary">{{ voiceHotkeyMessage }}</p>
        <details class="usage-hint-details">
          <summary>微信输入法使用步骤（点开查看）</summary>
          <ol>
            <li>语音设备选择 CABLE Input；</li>
            <li>在微信输入法的语音设置里，把麦克风设为 CABLE Output；若没有这个选项，把系统默认录音设备设为 CABLE Output；</li>
            <li>快捷键选"微信输入法"预设（左 Ctrl + 左 Win、按住说话，并开启自动切换输入法）；</li>
            <li>在目标应用的文本框内切换到微信输入法（看任务栏输入指示器确认）；</li>
            <li>按住遥控器语音键约半秒以上再说话，松开后等待文字出现（需要联网）。快速点按不出文字是微信输入法自己的最短按住要求，不是故障。遥控器语音键自带的 F5 按键会被应用自动屏蔽，物理键盘的 F5 不受影响。</li>
          </ol>
        </details>
        <details class="usage-hint-details">
          <summary>Typeless 等单次触发工具使用步骤（点开查看）</summary>
          <ol>
            <li>语音设备选择 CABLE Input，并在该工具里把麦克风设为 CABLE Output；</li>
            <li>在该工具的设置里给"启动语音输入"设一个快捷键（建议用不与系统冲突的组合，如右 Alt）；</li>
            <li>回到这里点"录入自定义按键"，按一次同样的快捷键；</li>
            <li>触发方式选"单次触发"，并关闭"自动切到微信输入法"；</li>
            <li>按住遥控器语音键说话、松开结束：应用会在语音开始时点按一次快捷键、在音频送完后再点按一次结束。</li>
          </ol>
        </details>
      </article>

      <article class="card">
        <div class="card-title-row">
          <div>
            <h2>语音设备</h2>
            <p class="muted">选择语音写入的设备。使用微信输入法请选 CABLE Input。</p>
          </div>
          <button
            class="secondary-button"
            type="button"
            :disabled="scanningAudio || audioBusy || !runtime?.platform.windowsApiAvailable"
            @click="scanAudio()"
          >
            {{ scanningAudio ? "读取中…" : "刷新设备列表" }}
          </button>
        </div>

        <div class="status-panel" aria-live="polite">
          <div class="status-copy">
            <div class="status-heading">
              <span class="status-dot" :class="audioTone"></span>
              <strong>{{ audio.selectedEndpointName ?? audioPhaseLabel(audio.phase) }}</strong>
            </div>
            <small>{{ audioDetail }}</small>
          </div>
        </div>

        <p class="muted scan-summary">{{ audioMessage }}</p>
        <div v-if="audioEndpoints.length" class="endpoint-select-row">
          <button
            class="secondary-button"
            type="button"
            @click="showEndpointList = !showEndpointList"
          >
            {{ showEndpointList ? "收起列表" : audio.selectedEndpointId ? "更换设备" : "选择设备" }}
          </button>
          <span v-if="!showEndpointList" class="muted endpoint-count">
            共 {{ audioEndpoints.length }} 个设备可选
          </span>
        </div>
        <ul v-if="showEndpointList && audioEndpoints.length" class="device-list endpoint-list">
          <li v-for="endpoint in audioEndpoints" :key="endpoint.id">
            <div>
              <strong>{{ endpoint.name }}</strong>
              <small>{{ endpoint.isVirtualCableCandidate ? "推荐（微信输入法等语音工具使用）" : "其他音频设备" }}</small>
            </div>
            <button
              type="button"
              :disabled="audioBusy || Boolean(selectingEndpointId) || audio.selectedEndpointId === endpoint.id"
              @click="chooseAudioEndpoint(endpoint)"
            >
              {{
                selectingEndpointId === endpoint.id
                  ? "正在启用…"
                  : audio.selectedEndpointId === endpoint.id
                    ? "当前设备"
                    : "选择"
              }}
            </button>
          </li>
        </ul>

        <div class="setting-list compact two-col">
          <div class="setting-row">
            <strong>语音设备</strong>
            <span>{{ wasapiReady ? audioPhaseLabel(audio.phase) : "待选择" }}</span>
          </div>
        </div>

        <div v-if="audioScanComplete && !virtualCableInstalled" class="info-callout warning vb-cable-callout">
          <div>
            <strong>需要安装 VB-CABLE</strong>
            <p>由 VB-Audio 提供的免费虚拟声卡。安装需要管理员权限，完成后需重启电脑。</p>
          </div>
          <div class="button-row">
            <button class="primary-button" type="button" :disabled="openingVbCablePage" @click="openVbCablePage">
              {{ openingVbCablePage ? "正在打开…" : "打开官方下载页" }}
            </button>
            <button class="secondary-button" type="button" :disabled="scanningAudio" @click="scanAudio()">
              重新检测
            </button>
          </div>
        </div>
        <div v-else class="info-callout" :class="{ warning: !wasapiReady }">
          {{
            wasapiReady
              ? "语音设备已就绪。"
              : virtualCableInstalled
                ? "已检测到 VB-CABLE。这里选择 CABLE Input；在微信输入法的语音设置里选择 CABLE Output。"
                : "正在检测 VB-CABLE…"
          }}
        </div>
      </article>
    </div>
  </section>
</template>
