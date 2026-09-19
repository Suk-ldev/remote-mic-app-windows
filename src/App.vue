<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import Sidebar from "./components/Sidebar.vue";
import { getRuntimeSnapshot, type RuntimeSnapshot } from "./lib/bridge";
import { reportFrontendEvent } from "./lib/frontend-diagnostics";
import { useAppUpdate } from "./lib/app-update";
import { loadReadinessPreferences, useReadiness } from "./lib/readiness";
import type { PageId } from "./navigation";
import { visibleNavigationItems } from "./navigation";
import AboutPage from "./pages/AboutPage.vue";
import ButtonsPage from "./pages/ButtonsPage.vue";
import ConnectionPage from "./pages/ConnectionPage.vue";
import PermissionsPage from "./pages/PermissionsPage.vue";
import PresetsPage from "./pages/PresetsPage.vue";
import ReadinessPage from "./pages/ReadinessPage.vue";

const activePage = ref<PageId>("readiness");
const { completed: readinessCompleted } = useReadiness();
/** 用户自己点过导航之后，启动逻辑不再改动当前页面。 */
let userNavigated = false;
const runtime = ref<RuntimeSnapshot | null>(null);
const loadError = ref("");
const { bannerVisible, info: updateInfo, dismissBanner, runStartupSilentCheck } = useAppUpdate();
let runtimePollTimer: ReturnType<typeof setInterval> | undefined;
let updateCheckTimer: ReturnType<typeof setTimeout> | undefined;
let initialRuntimeReported = false;

const activeComponent = computed(() => ({
  readiness: ReadinessPage,
  buttons: ButtonsPage,
  presets: PresetsPage,
  connection: ConnectionPage,
  permissions: PermissionsPage,
  about: AboutPage,
})[activePage.value]);

const navItems = computed(() =>
  visibleNavigationItems({
    readinessCompleted: readinessCompleted.value,
    activePage: activePage.value,
  }),
);

function selectPage(page: PageId): void {
  userNavigated = true;
  activePage.value = page;
}

// 横幅不在"关于"页重复显示（页面内已有完整更新面板）。
const updateBannerVisible = computed(
  () => bannerVisible.value && activePage.value !== "about",
);

function showUpdatePage(): void {
  selectPage("about");
}

onMounted(async () => {
  // 准备清单已整体完成过的用户不再从"准备"页进入（该页已收进关于）。
  void loadReadinessPreferences().then(() => {
    if (readinessCompleted.value && !userNavigated && activePage.value === "readiness") {
      activePage.value = "buttons";
    }
  });
  const refreshRuntime = async () => {
    try {
      runtime.value = await getRuntimeSnapshot();
      loadError.value = "";
      if (!initialRuntimeReported) {
        reportFrontendEvent({
          event: "runtime_snapshot",
          phase: "completed",
          result: "passed",
          reason: "initial_ipc_ready",
        });
        initialRuntimeReported = true;
      }
    } catch (error) {
      loadError.value = error instanceof Error ? error.message : String(error);
      if (!initialRuntimeReported) {
        reportFrontendEvent({
          event: "runtime_snapshot",
          phase: "completed",
          result: "failed",
          reason: "initial_ipc_failed",
        });
        initialRuntimeReported = true;
      }
    }
  };
  await refreshRuntime();
  runtimePollTimer = setInterval(() => {
    void refreshRuntime();
  }, 1_000);
  // 启动静默检查更新：延迟 3 秒避开 BLE 恢复/设置加载的启动高峰；
  // 失败完全无声（app-update.ts 内回落 idle，不打扰主功能）。
  updateCheckTimer = setTimeout(() => {
    void runStartupSilentCheck();
  }, 3_000);
});

onUnmounted(() => {
  if (runtimePollTimer) clearInterval(runtimePollTimer);
  if (updateCheckTimer) clearTimeout(updateCheckTimer);
});
</script>

<template>
  <div class="app-shell">
    <Sidebar :active-page="activePage" :items="navItems" @select="selectPage" />
    <main class="content">
      <div v-if="loadError" class="error-banner">无法读取运行状态：{{ loadError }}</div>
      <div v-if="updateBannerVisible" class="update-banner">
        <span>发现新版本 {{ updateInfo?.version }}</span>
        <button type="button" class="link-button" @click="showUpdatePage">查看</button>
        <button type="button" class="link-button" aria-label="忽略此提醒" @click="dismissBanner">
          ×
        </button>
      </div>
      <component :is="activeComponent" :runtime="runtime" @navigate="selectPage" />
    </main>
  </div>
</template>
