import { describe, expect, it } from "vitest";
import { decodeApplicationInfo } from "./application-info";

describe("application_info 边界解码", () => {
  it("接受完整的应用信息", () => {
    expect(
      decodeApplicationInfo({
        name: "Unimail",
        version: "0.1.0",
        platform: "windows",
        capabilities: ["offline-shell"],
      }),
    ).toEqual({
      name: "Unimail",
      version: "0.1.0",
      platform: "windows",
      capabilities: ["offline-shell"],
    });
  });

  it.each([
    null,
    {},
    { name: "Unimail", version: "0.1.0", platform: 1, capabilities: [] },
    { name: "Unimail", version: "0.1.0", platform: "windows", capabilities: [1] },
  ])("拒绝无效载荷 %#", (payload) => {
    expect(() => decodeApplicationInfo(payload)).toThrow(TypeError);
  });
});
