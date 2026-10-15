import { createApp, watchEffect } from "vue";
import { createPinia } from "pinia";
import { usePreferredDark } from "@vueuse/core";
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
});

disableBrowserContextMenu();

app.mount("#app");
