import { describe, expect, it } from "vitest";
import { navigationItems } from "./navigation";

describe("Windows navigation", () => {
  // "准备"是 Windows 版独有的首启清单页（Mac 原版没有），排在 Mac 派生的
  // 四页之前；其余顺序仍与 Mac 一致。
  it("keeps the approved Mac-derived page order without empty entries", () => {
    expect(navigationItems.map((item) => item.id)).toEqual([
      "readiness",
      "buttons",
      "connection",
      "permissions",
      "about",
    ]);
    expect(navigationItems.every((item) => item.label.length > 0)).toBe(true);
  });
});
