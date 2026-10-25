import { createApp, watchEffect } from "vue";
import { createPinia } from "pinia";
import { usePreferredDark } from "@vueuse/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
// Every font ships with the app from these packages; nothing is fetched at runtime, so the
// app looks the same offline. Each file covers all of the font's scripts (Latin, Cyrillic,
// Greek, Vietnamese), loaded only when a character from it appears.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/400-italic.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "@fontsource/ibm-plex-sans/700.css";
import "@fontsource/outfit/400.css";
import "@fontsource/outfit/500.css";
import "@fontsource/outfit/600.css";
import "@fontsource/outfit/700.css";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/400-italic.css";
import "@fontsource/jetbrains-mono/500.css";
import "@fontsource/jetbrains-mono/600.css";
import "@fontsource/jetbrains-mono/700.css";
// The second bundled choice for the diff font (settings.json `diff.fontFamily`).
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/400-italic.css";
import "@/assets/index.css";
import App from "./App.vue";
import { useSettingsStore } from "@/stores/settings";
import { disableBrowserContextMenu } from "@/lib/browserDefaults";
import { DIFF_COLORS, diffColorVars } from "@/lib/diff/colors";

// Dark is the default, so paint dark before settings arrive to avoid a light flash.
document.documentElement.classList.add("dark");

const app = createApp(App);
app.use(createPinia());

const settings = useSettingsStore();
const preferredDark = usePreferredDark();

watchEffect(() => {
  const theme = settings.theme;
  const dark = theme === "dark" || (theme === "system" && preferredDark.value);
  document.documentElement.classList.toggle("dark", dark);
  // The window and the webview behind the page take the theme's window colour, so whatever the
  // webview paints before the page has drawn (WebView2 paints white by default) matches it.
  // `tauri.conf.json` starts them dark, the default theme.
  const colour = getComputedStyle(document.documentElement).getPropertyValue("--window").trim();
  if (colour) {
    void getCurrentWindow().setBackgroundColor(colour).catch(() => {});
    void getCurrentWebview().setBackgroundColor(colour).catch(() => {});
  }
});

// `appearance.zoom`: the whole page larger or smaller, pixel sizes included, like a browser's zoom.
watchEffect(() => {
  const percent = settings.settings?.appearance?.zoom ?? 100;
  const factor = Math.min(200, Math.max(50, percent)) / 100;
  void getCurrentWebview().setZoom(factor).catch(() => {});
});

// `diff.addBgDark` and the other diff colours: the current theme's saved colours, set on <body>
// over the theme's own on <html>.
watchEffect(() => {
  const theme = settings.theme;
  const dark = theme === "dark" || (theme === "system" && preferredDark.value);
  const diff = settings.settings?.diff;
  const vars = diffColorVars((setting) => diff?.[setting], dark);
  for (const { cssVar } of DIFF_COLORS) {
    const value = vars[cssVar];
    if (value) document.body.style.setProperty(cssVar, value);
    else document.body.style.removeProperty(cssVar);
  }
});

disableBrowserContextMenu();

app.mount("#app");
