import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getMailRemoteImage, type RemoteImageResultV1 } from "../../lib/ipc/mail-reader";
import { SafeHtmlMessage } from "./SafeHtmlMessage";
import { sanitizeMailHtml } from "./sanitize-mail-html";

vi.mock("../../lib/ipc/mail-reader", () => ({ getMailRemoteImage: vi.fn() }));

describe("安全 HTML 邮件渲染", () => {
  beforeEach(() => {
    vi.mocked(getMailRemoteImage).mockReset();
  });
  afterEach(cleanup);

  it("移除活动内容、危险链接和远程图片源", () => {
    const result = sanitizeMailHtml(`
      <script>window.evil = true</script>
      <form action="https://evil.test"><input name="secret"></form>
      <svg><script>alert(1)</script></svg>
      <math><mtext>危险数学内容</mtext></math>
      <iframe src="https://frame.example.test"></iframe>
      <video src="https://media.example.test/a.mp4"></video>
      <img src="https://tracker.example.test/pixel.gif" srcset="https://tracker.example.test/2x.gif 2x" onerror="alert(1)" style="background:url(https://tracker.example.test/css.gif)" alt="跟踪图片">
      <a href="javascript:alert(1)">危险链接</a>
      <a href="https://safe.example.test/path">安全链接</a>
    `);

    expect(result.document).not.toContain("<script");
    expect(result.document).not.toContain("<form");
    expect(result.document).not.toContain("<svg");
    expect(result.document).not.toContain("<math");
    expect(result.document).not.toContain("<iframe");
    expect(result.document).not.toContain("<video");
    expect(result.document).not.toContain("srcset");
    expect(result.document).not.toContain("onerror");
    expect(result.document).not.toContain("background:url");
    expect(result.document).not.toContain("tracker.example.test");
    expect(result.document).not.toContain("javascript:");
    expect(result.document).toContain("default-src 'none'");
    expect(result.blockedImageCount).toBe(1);
    expect(result.remoteImages).toEqual([
      { alt: "跟踪图片", url: "https://tracker.example.test/pixel.gif" },
    ]);
    expect(result.links).toEqual([{ label: "安全链接", url: "https://safe.example.test/path" }]);
  });

  it("拒绝绕过 IPC 解码器注入的远程或 SVG 图片源", () => {
    const html = '<img src="https://images.example.test/a.png" alt="示例图片">';
    const remote = sanitizeMailHtml(
      html,
      new Map([["https://images.example.test/a.png", "https://tracker.example.test/pixel.png"]]),
    );
    const svg = sanitizeMailHtml(
      html,
      new Map([
        [
          "https://images.example.test/a.png",
          "data:image/svg+xml;base64,PHN2ZyBvbmxvYWQ9YWxlcnQoMSk+PC9zdmc+",
        ],
      ]),
    );
    expect(remote.document).not.toContain("tracker.example.test");
    expect(svg.document).not.toContain("image/svg+xml");
    expect(remote.blockedImageCount).toBe(1);
    expect(svg.blockedImageCount).toBe(1);
  });

  it("只在可信 React 区域发出外部链接确认动作", () => {
    const onExternalLink = vi.fn();
    render(
      <SafeHtmlMessage
        key="00000000-0000-4000-8000-000000000001"
        messageId="00000000-0000-4000-8000-000000000001"
        html={'<a href="https://safe.example.test/path">查看详情</a>'}
        onExternalLink={onExternalLink}
      />,
    );

    const frame = screen.getByTitle("邮件 HTML 正文");
    expect(frame.getAttribute("sandbox")).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "查看详情" }));
    expect(onExternalLink).toHaveBeenCalledWith("https://safe.example.test/path");
  });

  it("只有显式批准后才请求图片，并在切换邮件后重新阻止", async () => {
    const dataUrl = "data:image/png;base64,iVBORw0KGgo=";
    vi.mocked(getMailRemoteImage).mockResolvedValue({ mediaType: "image/png", dataUrl });
    const view = render(
      <SafeHtmlMessage
        messageId="00000000-0000-4000-8000-000000000001"
        html={'<img src="https://images.example.test/a.png" alt="示例图片">'}
        onExternalLink={vi.fn()}
      />,
    );

    expect(getMailRemoteImage).not.toHaveBeenCalled();
    expect(
      within(view.container).getByTitle("邮件 HTML 正文").getAttribute("srcdoc"),
    ).not.toContain("images.example.test");
    fireEvent.click(screen.getByRole("button", { name: "显示本邮件图片" }));
    await waitFor(() =>
      expect(within(view.container).getByTitle("邮件 HTML 正文").getAttribute("srcdoc")).toContain(
        dataUrl,
      ),
    );
    expect(getMailRemoteImage).toHaveBeenCalledWith(
      "00000000-0000-4000-8000-000000000001",
      "https://images.example.test/a.png",
    );

    view.rerender(
      <SafeHtmlMessage
        key="00000000-0000-4000-8000-000000000002"
        messageId="00000000-0000-4000-8000-000000000002"
        html={'<img src="https://images.example.test/a.png" alt="示例图片">'}
        onExternalLink={vi.fn()}
      />,
    );
    await waitFor(() =>
      expect(
        within(view.container).getByTitle("邮件 HTML 正文").getAttribute("srcdoc"),
      ).not.toContain(dataUrl),
    );
    expect(screen.getByRole("button", { name: "显示本邮件图片" })).toBeEnabled();
  });

  it("切换邮件时忽略尚未完成的图片结果", async () => {
    let resolveImage!: (value: RemoteImageResultV1) => void;
    const pendingImage = new Promise<RemoteImageResultV1>((resolve) => {
      resolveImage = resolve;
    });
    vi.mocked(getMailRemoteImage).mockReturnValue(pendingImage);
    const view = render(
      <SafeHtmlMessage
        key="00000000-0000-4000-8000-000000000001"
        messageId="00000000-0000-4000-8000-000000000001"
        html={'<img src="https://images.example.test/a.png" alt="示例图片">'}
        onExternalLink={vi.fn()}
      />,
    );
    fireEvent.click(within(view.container).getByRole("button", { name: "显示本邮件图片" }));
    view.rerender(
      <SafeHtmlMessage
        key="00000000-0000-4000-8000-000000000002"
        messageId="00000000-0000-4000-8000-000000000002"
        html={'<img src="https://images.example.test/b.png" alt="另一张图片">'}
        onExternalLink={vi.fn()}
      />,
    );
    resolveImage({ mediaType: "image/png", dataUrl: "data:image/png;base64,iVBORw0KGgo=" });

    await waitFor(() =>
      expect(
        within(view.container).getByTitle("邮件 HTML 正文").getAttribute("srcdoc"),
      ).not.toContain("data:image/png"),
    );
    expect(screen.getByRole("button", { name: "显示本邮件图片" })).toBeEnabled();
  });
});
