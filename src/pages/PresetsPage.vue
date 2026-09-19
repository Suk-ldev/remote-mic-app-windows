<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import {
  applyMappingPreset,
  exportButtonMappingConfiguration,
  getActiveAppProfile,
  getAppProfiles,
  getInjectionHoldMs,
  importButtonMappingConfiguration,
  listMappingPresets,
  resetButtonMappings,
  saveAppProfiles,
  setInjectionHoldMs,
  type AppProfileBindings,
  type MappingPresetInfo,
  type RuntimeSnapshot,
} from "../lib/bridge";
import type { PageId } from "../navigation";

defineProps<{ runtime: RuntimeSnapshot | null }>();
const emit = defineEmits<{ navigate: [PageId] }>();

const mappingPresets = ref<MappingPresetInfo[]>([]);
const selectedPreset = ref("");
const injectionHoldMs = ref(30);
const appProfiles = ref<AppProfileBindings>({ enabled: false, bindings: {} });
const activeProfile = ref<string | null>(null);
const newBindingProcess = ref("");
const newBindingPreset = ref("");
const busy = ref(false);
const statusMessage = ref<string | null>(null);
let activeProfileTimer: ReturnType<typeof setInterval> | undefined;
let unmounted = false;

const selectedPresetNote = computed(
  () => mappingPresets.value.find((preset) => preset.id === selectedPreset.value)?.note ?? "",
);

const appProfileRows = computed(() =>
  Object.entries(appProfiles.value.bindings).map(([process, preset]) => ({
    process,
    preset,
    presetName: mappingPresets.value.find((item) => item.id === preset)?.name ?? preset,
  })),
);

const activeProfileName = computed(() =>
  activeProfile.value
    ? (mappingPresets.value.find((item) => item.id === activeProfile.value)?.name ??
      activeProfile.value)
    : null,
);

async function applyPreset(): Promise<void> {
  const preset = mappingPresets.value.find((item) => item.id === selectedPreset.value);
  if (!preset) return;
  busy.value = true;
  statusMessage.value = null;
  try {
    await applyMappingPreset(preset.id);
    statusMessage.value = `已套用「${preset.name}」，未列出的按键回到未配置`;
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    busy.value = false;
  }
}

async function persistAppProfiles(next: AppProfileBindings): Promise<void> {
  busy.value = true;
  try {
    appProfiles.value = await saveAppProfiles(next);
    activeProfile.value = await getActiveAppProfile();
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    busy.value = false;
  }
}

async function toggleAppProfiles(enabled: boolean): Promise<void> {
  await persistAppProfiles({ ...appProfiles.value, enabled });
}

async function addAppBinding(): Promise<void> {
  const process = newBindingProcess.value.trim();
  const preset = newBindingPreset.value || mappingPresets.value[0]?.id;
  if (!process || !preset) return;
  await persistAppProfiles({
    enabled: true,
    bindings: { ...appProfiles.value.bindings, [process]: preset },
  });
  newBindingProcess.value = "";
}

async function removeAppBinding(process: string): Promise<void> {
  const bindings = { ...appProfiles.value.bindings };
  delete bindings[process];
  await persistAppProfiles({ ...appProfiles.value, bindings });
}

async function applyInjectionHold(): Promise<void> {
  const wanted = Math.min(Math.max(Math.round(injectionHoldMs.value) || 0, 0), 1000);
  try {
    injectionHoldMs.value = await setInjectionHoldMs(wanted);
    statusMessage.value = `按键保持时长已设为 ${injectionHoldMs.value} 毫秒`;
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  }
}

async function exportConfiguration(): Promise<void> {
  busy.value = true;
  statusMessage.value = null;
  try {
    const exported = await exportButtonMappingConfiguration();
    if (exported) statusMessage.value = "按键映射配置已导出";
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    busy.value = false;
  }
}

async function importConfiguration(): Promise<void> {
  busy.value = true;
  statusMessage.value = null;
  try {
    const imported = await importButtonMappingConfiguration();
    if (!imported) return;
    statusMessage.value = "按键映射配置已导入并生效";
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    busy.value = false;
  }
}

async function restoreDefaults(): Promise<void> {
  busy.value = true;
  statusMessage.value = null;
  try {
    await resetButtonMappings();
    statusMessage.value = "已恢复默认（全部按键保持原始行为）";
  } catch (error) {
    statusMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    busy.value = false;
  }
}

onMounted(async () => {
  const presets = await listMappingPresets().catch(() => [] as MappingPresetInfo[]);
  if (unmounted) return;
  mappingPresets.value = presets;
  selectedPreset.value = presets[0]?.id ?? "";
  newBindingPreset.value = presets[0]?.id ?? "";
  void getAppProfiles()
    .then((profiles) => {
      if (!unmounted) appProfiles.value = profiles;
    })
    .catch(() => {});
  void getInjectionHoldMs()
    .then((millis) => {
      if (!unmounted) injectionHoldMs.value = millis;
    })
    .catch(() => {});
  // 轮询当前生效的方案：切换发生在监视线程里，界面只读状态做提示。
  activeProfileTimer = setInterval(() => {
    void getActiveAppProfile()
      .then((profile) => {
        if (!unmounted) activeProfile.value = profile;
      })
      .catch(() => {});
  }, 1_500);
});

onUnmounted(() => {
  unmounted = true;
  if (activeProfileTimer) clearInterval(activeProfileTimer);
});
</script>

<template>
  <section class="presets-page">
    <header class="page-header">
      <div>
        <h1>方案</h1>
        <p class="muted">整套按键配置的套用、切换与备份。单个按键怎么配，在「按键」页。</p>
      </div>
      <button class="secondary-button" type="button" @click="emit('navigate', 'buttons')">
        去配按键
      </button>
    </header>

    <article class="card settings-card">
      <div class="settings-row">
        <div class="settings-copy">
          <strong>预设方案</strong>
          <p class="muted">套用会整体替换现有映射，未列出的按键回到未配置。</p>
          <p v-if="selectedPresetNote" class="muted settings-note">{{ selectedPresetNote }}</p>
        </div>
        <div class="settings-control">
          <select
            id="mapping-preset"
            v-model="selectedPreset"
            class="preset-bar-select"
            aria-label="预设方案"
            :disabled="busy || !mappingPresets.length"
          >
            <option v-for="preset in mappingPresets" :key="preset.id" :value="preset.id">
              {{ preset.name }}
            </option>
          </select>
          <button
            class="primary-button"
            type="button"
            :disabled="busy || !selectedPreset"
            @click="applyPreset"
          >
            套用
          </button>
        </div>
      </div>
    </article>

    <article class="card settings-card">
      <div class="settings-row">
        <div class="settings-copy">
          <strong>按应用自动切换</strong>
          <p class="muted">
            切到绑定的应用时自动套用方案，离开后回到你保存的配置；自动切换不会改写保存的映射。
          </p>
        </div>
        <div class="settings-control">
          <input
            id="app-profile-toggle"
            :checked="appProfiles.enabled"
            type="checkbox"
            class="toggle-input"
            aria-label="按应用自动切换"
            :disabled="busy || !mappingPresets.length"
            @change="toggleAppProfiles(($event.target as HTMLInputElement).checked)"
          />
        </div>
      </div>

      <div v-if="appProfiles.enabled" class="app-profile-panel">
        <p v-if="activeProfileName" class="muted app-profile-active">
          当前由「{{ activeProfileName }}」方案接管；切回其他应用即恢复你保存的配置。
        </p>
        <ul v-if="appProfileRows.length" class="app-profile-list">
          <li v-for="row in appProfileRows" :key="row.process" class="app-profile-row">
            <code>{{ row.process }}</code>
            <span class="muted">→ {{ row.presetName }}</span>
            <button
              class="sequence-remove"
              type="button"
              :disabled="busy"
              :title="`删除 ${row.process} 的绑定`"
              @click="removeAppBinding(row.process)"
            >
              ✕
            </button>
          </li>
        </ul>
        <p v-else class="muted">还没有绑定。填入进程名（如 chrome）再选方案即可。</p>
        <div class="text-action-row">
          <input
            v-model="newBindingProcess"
            class="text-action-input"
            type="text"
            placeholder="进程名，例如 chrome"
            :disabled="busy"
            @keyup.enter="addAppBinding"
          />
          <select v-model="newBindingPreset" class="preset-bar-select" aria-label="绑定的方案" :disabled="busy">
            <option v-for="preset in mappingPresets" :key="preset.id" :value="preset.id">
              {{ preset.name }}
            </option>
          </select>
          <button class="chip" type="button" :disabled="busy || !newBindingProcess" @click="addAppBinding">
            添加绑定
          </button>
        </div>
      </div>
    </article>

    <article class="card settings-card">
      <div class="settings-row">
        <div class="settings-copy">
          <strong>按键保持</strong>
          <p class="muted">
            注入的按下与松开之间保持这么久。目标应用（游戏、部分 Electron 程序）漏识别时调高到 50。
          </p>
        </div>
        <div class="settings-control">
          <input
            id="injection-hold"
            v-model.number="injectionHoldMs"
            class="delay-input hold-input"
            type="number"
            min="0"
            max="1000"
            aria-label="按键保持时长（毫秒）"
            :disabled="busy"
            @change="applyInjectionHold"
          />
          <span class="muted">毫秒</span>
        </div>
      </div>
    </article>

    <article class="card settings-card">
      <div class="settings-row">
        <div class="settings-copy">
          <strong>配置备份</strong>
          <p class="muted">导出的是带版本号的映射文件；恢复默认会让全部按键回到原始行为。</p>
        </div>
        <div class="settings-control">
          <button class="secondary-button" type="button" :disabled="busy" @click="importConfiguration">
            导入配置…
          </button>
          <button class="secondary-button" type="button" :disabled="busy" @click="exportConfiguration">
            导出配置…
          </button>
          <button class="secondary-button" type="button" :disabled="busy" @click="restoreDefaults">
            恢复默认
          </button>
        </div>
      </div>
    </article>

    <p v-if="statusMessage" class="operation-message">{{ statusMessage }}</p>
  </section>
</template>
