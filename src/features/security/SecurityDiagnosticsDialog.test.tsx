import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { getSecurityDiagnostics } from "../../lib/ipc/security-diagnostics";
import type { SecurityDiagnosticsV1 } from "../../lib/ipc/security-diagnostics";
import { SecurityDiagnosticsDialog } from "./SecurityDiagnosticsDialog";

vi.mock("../../lib/ipc/security-diagnostics", () => ({ getSecurityDiagnostics: vi.fn() }));

const diagnostic: SecurityDiagnosticsV1 = {
  appVersion: "0.1.0",
  platform: "windows",
  online: true,
  storage: {
    ready: true,
    schemaVersion: 4,
    cipherAvailable: true,
    fts5Available: true,
    credentialStore: "windows",
    safeErrorCode: null,
  },
  providers: [
    {
      provider: "gmail",
      configured: true,
      accountCount: 2,
      connectedCount: 1,
      reconnectCount: 1,
    },
    {
      provider: "outlook",
      configured: false,
      accountCount: null,
      connectedCount: null,
      reconnectCount: null,
    },
    {
      provider: "qq",
      configured: true,
      accountCount: 0,
      connectedCount: 0,
      reconnectCount: 0,
    },
    {
      provider: "netease",
      configured: true,
      accountCount: 0,
      connectedCount: 0,
      reconnectCount: 0,
    },
  ],
};

describe("安全与诊断弹窗", () => {
  afterEach(cleanup);

  it("展示只含安全状态和服务商计数的可选择文本", async () => {
    vi.mocked(getSecurityDiagnostics).mockResolvedValueOnce(diagnostic);
    render(<SecurityDiagnosticsDialog onClose={vi.fn()} />);

    const text = await screen.findByLabelText("可选择的诊断文本");
    expect(text.textContent).toContain("应用版本：0.1.0");
    expect(text.textContent).toContain("Gmail：已配置；账户总数 2；正常数 1；需重连数 1");
    expect(text.textContent).toContain(
      "Outlook：未配置；账户总数 不可用；正常数 不可用；需重连数 不可用",
    );
    expect(text.textContent).not.toContain("@example.com");
    expect(text.textContent).not.toContain("C:\\");
  });

  it("支持 Escape 关闭", async () => {
    vi.mocked(getSecurityDiagnostics).mockResolvedValueOnce(diagnostic);
    const onClose = vi.fn();
    render(<SecurityDiagnosticsDialog onClose={onClose} />);
    await screen.findByLabelText("可选择的诊断文本");

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("把 Tab 焦点保持在弹窗内", async () => {
    vi.mocked(getSecurityDiagnostics).mockRejectedValueOnce(new Error("unavailable"));
    render(<SecurityDiagnosticsDialog onClose={vi.fn()} />);
    await screen.findByRole("alert");
    const close = screen.getByRole("button", { name: "关闭" });
    const retry = screen.getByRole("button", { name: "重试" });

    retry.focus();
    fireEvent.keyDown(window, { key: "Tab" });
    expect(document.activeElement).toBe(close);
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(retry);
  });

  it("失败后只展示通用提示并可重试", async () => {
    vi.mocked(getSecurityDiagnostics)
      .mockRejectedValueOnce(new Error("private failure"))
      .mockResolvedValueOnce(diagnostic);
    render(<SecurityDiagnosticsDialog onClose={vi.fn()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("暂时无法读取安全诊断");
    fireEvent.click(screen.getByRole("button", { name: "重试" }));
    await waitFor(() => expect(screen.getByLabelText("可选择的诊断文本")).toBeTruthy());
  });
});
