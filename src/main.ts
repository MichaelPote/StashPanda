import { createApp, watchEffect } from "vue";
import { createPinia } from "pinia";
import { usePreferredDark } from "@vueuse/core";
import "@/assets/index.css";
import App from "./App.vue";
import { useSettingsStore } from "@/stores/settings";

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

app.mount("#app");
