export type PageId = "readiness" | "buttons" | "connection" | "permissions" | "about";

export type NavIcon = "checklist" | "keyboard" | "link" | "shield" | "info";

export interface NavigationItem {
  id: PageId;
  label: string;
  /** 侧栏图标（形状对齐 macOS SF Symbols：checklist/keyboard/link/shield/info.circle）。 */
  icon: NavIcon;
}

export const navigationItems: NavigationItem[] = [
  { id: "readiness", label: "准备", icon: "checklist" },
  { id: "buttons", label: "按键", icon: "keyboard" },
  { id: "connection", label: "连接与语音", icon: "link" },
  { id: "permissions", label: "权限", icon: "shield" },
  { id: "about", label: "关于", icon: "info" },
];
