import { useEffect, useMemo, useRef, useState } from "react";
import { mailReaderContent } from "../../content/mail-reader.zh-CN";
import { getMailRemoteImage } from "../../lib/ipc/mail-reader";
import { sanitizeMailHtml } from "./sanitize-mail-html";

export function SafeHtmlMessage({
  messageId,
  html,
  onExternalLink,
}: {
  messageId: string;
  html: string;
  onExternalLink: (url: string) => void;
}) {
  const manifest = useMemo(() => sanitizeMailHtml(html), [html]);
  const [approvedImages, setApprovedImages] = useState<ReadonlyMap<string, string>>(new Map());
  const [loadingImages, setLoadingImages] = useState(false);
  const [imageError, setImageError] = useState(false);
  const requestGeneration = useRef(0);
  useEffect(() => {
    return () => {
      requestGeneration.current += 1;
    };
  }, []);
  const safe = useMemo(() => sanitizeMailHtml(html, approvedImages), [approvedImages, html]);

  async function showRemoteImages() {
    const generation = requestGeneration.current + 1;
    requestGeneration.current = generation;
    setLoadingImages(true);
    setImageError(false);
    try {
      const images: Array<{ dataUrl: string; url: string }> = [];
      for (const image of manifest.remoteImages) {
        const result = await getMailRemoteImage(messageId, image.url);
        images.push({ dataUrl: result.dataUrl, url: image.url });
      }
      if (requestGeneration.current === generation) {
        setApprovedImages(new Map(images.map((image) => [image.url, image.dataUrl])));
        setLoadingImages(false);
      }
    } catch {
      if (requestGeneration.current === generation) {
        setLoadingImages(false);
        setImageError(true);
      }
    }
  }

  return (
    <div className="safe-html-message">
      {safe.blockedImageCount > 0 && (
        <div className="remote-content-notice">
          <p>{mailReaderContent.blockedImages(safe.blockedImageCount)}</p>
          {manifest.remoteImages.length > 0 && approvedImages.size === 0 && (
            <button type="button" disabled={loadingImages} onClick={() => void showRemoteImages()}>
              {loadingImages
                ? mailReaderContent.loadingRemoteImages
                : mailReaderContent.showRemoteImages}
            </button>
          )}
          {imageError && <p role="alert">{mailReaderContent.remoteImagesUnavailable}</p>}
        </div>
      )}
      <iframe
        className="message-html-frame"
        sandbox=""
        srcDoc={safe.document}
        title="邮件 HTML 正文"
      />
      {safe.links.length > 0 && (
        <section className="message-links" aria-labelledby="message-links-heading">
          <h3 id="message-links-heading">{mailReaderContent.externalLinks}</h3>
          <ul>
            {safe.links.map((link, index) => (
              <li key={`${link.url}-${String(index)}`}>
                <button type="button" onClick={() => onExternalLink(link.url)}>
                  {link.label}
                </button>
                <small>{new URL(link.url).hostname}</small>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
