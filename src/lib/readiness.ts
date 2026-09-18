import { readonly, ref } from "vue";
import {
  getReadinessPreferences,
  setReadinessCompleted,
  setReadinessConfirmation,
} from "./bridge";

/**
 * 准备清单的用户侧状态。两处共用同一份响应式状态：
 * 准备页（确认按钮、完成判定）与外壳（侧栏是否收起"准备"）。
 */
const confirmedItems = ref<string[]>([]);
const completed = ref(false);
const busy = ref(false);
const errorMessage = ref("");
let loaded = false;

export async function loadReadinessPreferences(force = false): Promise<void> {
  if (loaded && !force) return;
  try {
    const preferences = await getReadinessPreferences();
    confirmedItems.value = preferences.confirmedItems;
    completed.value = preferences.completed;
    loaded = true;
    console.info(
      `feature=readiness event=preferences_loaded confirmed=${preferences.confirmedItems.length} completed=${preferences.completed}`,
    );
  } catch {
    // 读不到就按"全部未确认"走：清单只会多显示几项，不会误判为已完成。
    console.warn("feature=readiness event=preferences_load result=error reason=ipc_failed");
  }
}

export function isReadinessItemConfirmed(itemId: string): boolean {
  return confirmedItems.value.includes(itemId);
}

/** 手动确认/撤销某个准备项。失败时回滚并给出可见的错误文案。 */
export async function confirmReadinessItem(
  itemId: string,
  confirmed: boolean,
): Promise<void> {
  if (busy.value) return;
  busy.value = true;
  errorMessage.value = "";
  try {
    const preferences = await setReadinessConfirmation(itemId, confirmed);
    confirmedItems.value = preferences.confirmedItems;
    completed.value = preferences.completed;
    loaded = true;
    console.info(
      `feature=readiness event=item_confirmed item=${itemId} confirmed=${confirmed} result=ok`,
    );
  } catch {
    errorMessage.value = confirmed ? "确认没能保存，请重试。" : "撤销确认没能保存，请重试。";
    console.warn(
      `feature=readiness event=item_confirmed item=${itemId} confirmed=${confirmed} result=error reason=save_failed`,
    );
  } finally {
    busy.value = false;
  }
}

/**
 * 整体完成标记：置位后侧栏收起"准备"，入口移到"关于"。
 * 只在从未完成过时写一次；之后某项临时不满足（遥控器断开）不再改动它。
 */
export async function markReadinessCompleted(): Promise<void> {
  if (completed.value || busy.value) return;
  busy.value = true;
  try {
    const preferences = await setReadinessCompleted(true);
    confirmedItems.value = preferences.confirmedItems;
    completed.value = preferences.completed;
    console.info("feature=readiness event=completed result=ok");
  } catch {
    console.warn("feature=readiness event=completed result=error reason=save_failed");
  } finally {
    busy.value = false;
  }
}

export function useReadiness() {
  return {
    confirmedItems: readonly(confirmedItems),
    completed: readonly(completed),
    busy: readonly(busy),
    errorMessage: readonly(errorMessage),
    isReadinessItemConfirmed,
    confirmReadinessItem,
    markReadinessCompleted,
    loadReadinessPreferences,
  };
}

/** 测试用：清掉进程内缓存（模块级状态在同一个测试进程里会串场）。 */
export function resetReadinessState(): void {
  confirmedItems.value = [];
  completed.value = false;
  busy.value = false;
  errorMessage.value = "";
  loaded = false;
}
