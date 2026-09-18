import { describe, expect, it } from "vitest";
import { navigationItems, visibleNavigationItems } from "./navigation";

describe("Windows navigation", () => {
  // "准备"是 Windows 版独有的首启清单页（Mac 原版没有），排在 Mac 派生的
  // 四页之前；"方案"承接整套映射的套用与备份，紧跟"按键"。
  it("keeps the approved Mac-derived page order without empty entries", () => {
    expect(navigationItems.map((item) => item.id)).toEqual([
      "readiness",
      "buttons",
      "presets",
      "connection",
      "permissions",
      "about",
    ]);
    expect(navigationItems.every((item) => item.label.length > 0)).toBe(true);
  });

  it("准备清单完成前，侧栏保持完整", () => {
    const visible = visibleNavigationItems({
      readinessCompleted: false,
      activePage: "buttons",
    });
    expect(visible.map((item) => item.id)).toContain("readiness");
  });

  it("准备清单完成后收起“准备”，但正停在该页时保留", () => {
    expect(
      visibleNavigationItems({ readinessCompleted: true, activePage: "buttons" }).map(
        (item) => item.id,
      ),
    ).not.toContain("readiness");

    expect(
      visibleNavigationItems({ readinessCompleted: true, activePage: "readiness" }).map(
        (item) => item.id,
      ),
    ).toContain("readiness");
  });
});
