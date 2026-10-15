/**
 * The window behaves like an application, not a web page: right-clicking shows no browser menu
 * (Back, Reload, Inspect Element and the like). The app's own menus (reka-ui `ContextMenu`)
 * open as before, since they handle the event on their own element before it reaches `window`;
 * this listener only cancels the browser's menu and never stops the event. Text fields keep the
 * browser's menu, for Cut, Copy and Paste. The browser's keyboard shortcuts are turned off on
 * the Rust side (`src-tauri/src/webview.rs`); debug builds open the inspector from the
 * Developer menu.
 */
import { isEditableTarget } from "@/lib/files";

export function disableBrowserContextMenu(): void {
  window.addEventListener("contextmenu", (event: MouseEvent) => {
    if (!isEditableTarget(event.target)) event.preventDefault();
  });
}
