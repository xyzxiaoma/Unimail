import DOMPurify from "dompurify";
import { isSafeRasterDataUrl } from "../../lib/security/raster-data-url";

const allowedTags = [
  "a",
  "abbr",
  "article",
  "b",
  "blockquote",
  "br",
  "code",
  "div",
  "em",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "hr",
  "i",
  "li",
  "ol",
  "p",
  "pre",
  "section",
  "small",
  "span",
  "strong",
  "table",
  "tbody",
  "td",
  "th",
  "thead",
  "tr",
  "u",
  "ul",
] as const;

export type SafeHtmlResult = {
  document: string;
  links: Array<{ label: string; url: string }>;
  blockedImageCount: number;
  remoteImages: Array<{ alt: string; url: string }>;
};

function normalizeHttpUrl(value: string): string | null {
  try {
    const url = new URL(value);
    if ((url.protocol !== "https:" && url.protocol !== "http:") || url.username || url.password) {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}

function normalizeRemoteImageUrl(value: string): string | null {
  try {
    const url = new URL(value);
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.hash ||
      (url.port !== "" && url.port !== "443")
    ) {
      return null;
    }
    return url.toString();
  } catch {
    return null;
  }
}

export function sanitizeMailHtml(
  html: string,
  approvedImageSources: ReadonlyMap<string, string> = new Map(),
): SafeHtmlResult {
  const fragment = DOMPurify.sanitize(html, {
    ALLOWED_TAGS: [...allowedTags, "img"],
    ALLOWED_ATTR: ["alt", "colspan", "dir", "height", "href", "rowspan", "src", "title", "width"],
    FORBID_ATTR: ["style", "srcset"],
    FORBID_TAGS: [
      "base",
      "button",
      "embed",
      "form",
      "frame",
      "iframe",
      "input",
      "link",
      "math",
      "meta",
      "object",
      "script",
      "select",
      "svg",
      "textarea",
      "video",
    ],
    RETURN_DOM_FRAGMENT: true,
  });
  const links: SafeHtmlResult["links"] = [];
  for (const anchor of fragment.querySelectorAll("a")) {
    const href = anchor.getAttribute("href");
    const normalized = href ? normalizeHttpUrl(href) : null;
    if (normalized) {
      links.push({ label: anchor.textContent.trim() || normalized, url: normalized });
    }
    anchor.removeAttribute("href");
  }
  let blockedImageCount = 0;
  const remoteImages: SafeHtmlResult["remoteImages"] = [];
  const seenRemoteImages = new Set<string>();
  for (const image of fragment.querySelectorAll("img")) {
    const alt = image.getAttribute("alt")?.trim() || "远程图片";
    const source = image.getAttribute("src");
    const normalized = source ? normalizeRemoteImageUrl(source) : null;
    if (normalized && !seenRemoteImages.has(normalized) && remoteImages.length < 12) {
      seenRemoteImages.add(normalized);
      remoteImages.push({ alt, url: normalized });
    }
    const approvedSource = normalized ? approvedImageSources.get(normalized) : undefined;
    if (isSafeRasterDataUrl(approvedSource)) {
      image.setAttribute("src", approvedSource);
      continue;
    }
    blockedImageCount += 1;
    const replacement = document.createElement("span");
    replacement.textContent = alt || "[远程图片已阻止]";
    image.replaceWith(replacement);
  }
  const container = document.createElement("div");
  container.append(fragment);
  const csp =
    "default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; " +
    "font-src 'none'; media-src 'none'; frame-src 'none'; object-src 'none'; " +
    "base-uri 'none'; form-action 'none'";
  return {
    document: `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${csp}"><style>body{margin:0;padding:16px;color:#393235;font:14px/1.65 system-ui,sans-serif;overflow-wrap:anywhere}table,img{max-width:100%}img{height:auto}table{border-collapse:collapse}pre{white-space:pre-wrap}a{color:#8c4050;text-decoration:underline}</style></head><body>${container.innerHTML}</body></html>`,
    links,
    blockedImageCount,
    remoteImages,
  };
}
