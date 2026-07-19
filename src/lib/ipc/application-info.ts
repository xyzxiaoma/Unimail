import { applicationInfo, type ApplicationInfo } from "./bindings";
import { isRecord } from "./decode";

export type { ApplicationInfo } from "./bindings";

export function decodeApplicationInfo(value: unknown): ApplicationInfo {
  if (!isRecord(value)) {
    throw new TypeError("application_info 必须返回对象");
  }

  const { name, version, platform, capabilities } = value;
  if (
    typeof name !== "string" ||
    typeof version !== "string" ||
    typeof platform !== "string" ||
    !Array.isArray(capabilities) ||
    !capabilities.every((capability) => typeof capability === "string")
  ) {
    throw new TypeError("application_info 返回了无效数据");
  }

  return { name, version, platform, capabilities };
}

export async function getApplicationInfo(): Promise<ApplicationInfo> {
  return decodeApplicationInfo(await applicationInfo());
}
