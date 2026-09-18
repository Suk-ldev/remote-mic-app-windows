export type PageId =
  | "readiness"
  | "buttons"
  | "presets"
  | "connection"
  | "permissions"
  | "about";

export type NavIcon = "checklist" | "keyboard" | "sliders" | "link" | "shield" | "info";

export interface NavigationItem {
  id: PageId;
  label: string;
  /** 侧栏图标（形状对齐 macOS SF Symbols：checklist/keyboard/slider.horizontal.3/link/shield/info.circle）。 */
  icon: NavIcon;
}

export const navigationItems: NavigationItem[] = [
  { id: "readiness", label: "准备", icon: "checklist" },
  { id: "buttons", label: "按键", icon: "keyboard" },
  { id: "presets", label: "方案", icon: "sliders" },
  { id: "connection", label: "连接与语音", icon: "link" },
  { id: "permissions", label: "权限", icon: "shield" },
  { id: "about", label: "关于", icon: "info" },
];

/**
 * 侧栏实际显示的页面：准备清单整体完成后收起"准备"（入口移到关于页），
 * 但正停在这一页时仍然保留——不让用户脚下的导航项凭空消失。
 */
export function visibleNavigationItems(options: {
  readinessCompleted: boolean;
  activePage: PageId;
}): NavigationItem[] {
  if (!options.readinessCompleted) return navigationItems;
  return navigationItems.filter(
    (item) => item.id !== "readiness" || options.activePage === "readiness",
  );
}
