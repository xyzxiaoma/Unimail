import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { decodeStorageCommandError, getStorageStatus } from "./lib/ipc/storage-status";

vi.mock("./lib/ipc/application-info", () => ({
  getApplicationInfo: vi.fn().mockRejectedValue(new Error("IPC unavailable in test")),
}));

vi.mock("./lib/ipc/storage-status", () => ({
  decodeStorageCommandError: vi.fn(),
  getStorageStatus: vi.fn(),
}));

const mockedDecodeStorageCommandError = vi.mocked(decodeStorageCommandError);
const mockedGetStorageStatus = vi.mocked(getStorageStatus);

describe("Unimail 基础界面", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
    mockedGetStorageStatus.mockReset();
    mockedGetStorageStatus.mockRejectedValue(new Error("IPC unavailable in test"));
    mockedDecodeStorageCommandError.mockReset();
    mockedDecodeStorageCommandError.mockImplementation(() => {
      throw new TypeError("invalid test rejection");
    });
  });

  it("展示中文三栏空状态和桌面状态占位", () => {
    render(<App />);

    expect(screen.getByRole("navigation", { name: "邮件文件夹" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "收件箱", level: 1 })).toBeTruthy();
    expect(screen.getByText("收件箱空空如也")).toBeTruthy();
    expect(screen.getByText("选择一封邮件开始阅读")).toBeTruthy();
    expect(screen.getByText("正在检查加密存储")).toBeTruthy();
    expect(screen.getByText("等待添加账户")).toBeTruthy();
  });

  it("可通过按钮打开写邮件占位并用 Escape 关闭", () => {
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: /写邮件/ }));
    expect(screen.getByRole("dialog", { name: "撰写邮件" })).toBeTruthy();
    expect(screen.getByPlaceholderText("邮件编辑功能将在后续版本开放")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "撰写邮件" })).toBeNull();
  });

  it("同步占位会提供非破坏性的状态反馈", () => {
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "同步邮件" }));
    expect(screen.getByText("尚无可同步账户")).toBeTruthy();
  });

  it("展示经过解码的加密存储就绪状态", async () => {
    mockedGetStorageStatus.mockResolvedValue({
      ready: true,
      schemaVersion: 1,
      cipherAvailable: true,
      fts5Available: true,
      credentialStore: "windows",
    });

    render(<App />);

    expect(await screen.findByText("加密存储已就绪 · Schema 1")).toBeTruthy();
  });

  it("不会把未经验证的命令拒绝内容显示给用户", async () => {
    const leakedPath = "C:\\Users\\someone\\mail.db";
    mockedGetStorageStatus.mockRejectedValue({
      code: "database_open_failed",
      message: leakedPath,
      retryable: true,
    });

    render(<App />);

    expect(await screen.findByText("无法读取加密存储状态")).toBeTruthy();
    expect(screen.queryByText(leakedPath)).toBeNull();
  });
});
