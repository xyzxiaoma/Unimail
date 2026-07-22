import { getCurrentWindow } from "@tauri-apps/api/window";

export async function protectDesktopClose(flush: () => Promise<void>): Promise<() => void> {
  const currentWindow = getCurrentWindow();
  return currentWindow.onCloseRequested((event) => {
    event.preventDefault();
    void flush()
      .then(() => currentWindow.destroy())
      .catch(() => {
        /* Keep the window open when the latest local draft cannot be persisted. */
      });
  });
}
