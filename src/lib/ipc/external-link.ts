import { openConfirmedExternalUrl } from "./bindings";

export async function openExternalLink(url: string): Promise<void> {
  await openConfirmedExternalUrl(url);
}
