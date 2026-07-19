import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";

vi.mock("./lib/ipc/application-info", () => ({
  getApplicationInfo: vi.fn().mockRejectedValue(new Error("IPC unavailable in test")),
}));

describe("Unimail 基础界面", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("展示中文三栏空状态和桌面状态占位", () => {
    render(<App />);

    expect(screen.getByRole("navigation", { name: "邮件文件夹" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "收件箱", level: 1 })).toBeTruthy();
    expect(screen.getByText("收件箱空空如也")).toBeTruthy();
    expect(screen.getByText("选择一封邮件开始阅读")).toBeTruthy();
    expect(screen.getByText("离线可用")).toBeTruthy();
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
});
