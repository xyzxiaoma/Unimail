import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { connectAuthorizationCodeAccount } from "../../lib/ipc/authorization-code-onboarding";
import { AuthorizationCodeOnboardingDialog } from "./AuthorizationCodeOnboardingDialog";

vi.mock("../../lib/ipc/authorization-code-onboarding", () => ({
  connectAuthorizationCodeAccount: vi.fn(),
  decodeAuthorizationCodeError: vi.fn(() => {
    throw new TypeError("invalid test error");
  }),
}));

const mockedConnect = vi.mocked(connectAuthorizationCodeAccount);

describe("AuthorizationCodeOnboardingDialog", () => {
  beforeEach(() => mockedConnect.mockReset());
  afterEach(cleanup);

  it("校验域名并提交 QQ 授权码", async () => {
    mockedConnect.mockResolvedValue({
      id: "account-qq",
      provider: "qq",
      email: "owner@qq.com",
      displayName: null,
      authState: "connected",
    });
    const onConnected = vi.fn();
    render(
      <AuthorizationCodeOnboardingDialog
        initialProvider="qq"
        reconnectAccount={null}
        onClose={vi.fn()}
        onConnected={onConnected}
      />,
    );
    fireEvent.change(screen.getByLabelText("邮箱地址"), {
      target: { value: "owner@163.com" },
    });
    fireEvent.change(screen.getByLabelText("IMAP/SMTP 授权码"), {
      target: { value: "private-code" },
    });
    fireEvent.click(screen.getByRole("button", { name: "连接邮箱" }));
    expect(await screen.findByText("请输入完整的 @qq.com 邮箱地址。")).toBeInTheDocument();
    expect(mockedConnect).not.toHaveBeenCalled();

    fireEvent.change(screen.getByLabelText("邮箱地址"), { target: { value: "owner@qq.com" } });
    fireEvent.click(screen.getByRole("button", { name: "连接邮箱" }));
    await waitFor(() =>
      expect(onConnected).toHaveBeenCalledWith(expect.objectContaining({ provider: "qq" })),
    );
    expect(mockedConnect).toHaveBeenCalledWith("qq", null, "owner@qq.com", "private-code");
  });
});
