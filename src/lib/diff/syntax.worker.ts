/**
 * Colours diff text off the main thread with Shiki. The main thread sends a file's path and runs
 * of lines (one hunk's old side, or its new side) and gets back, for every line, its tokens as
 * `[text, colour, fontStyle]`. A language or theme is loaded the first time it is asked for;
 * Shiki's JavaScript regular expression engine is used, so no WebAssembly is loaded.
 *
 * Only the languages below are offered. Tauri builds every asset into the program, and all of
 * Shiki's 346 grammars would add about 10 MB to it for languages hardly anyone opens.
 */
import { createHighlighterCore, type HighlighterCore, type LanguageInput, type ThemeInput } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";

/** The themes the viewer uses, one for each app theme. */
export type SyntaxTheme = "github-dark-default" | "github-light-default";

export interface SyntaxRequest {
  id: number;
  /** The file's path, which decides its language. */
  path: string;
  theme: SyntaxTheme;
  runs: string[][];
}

/** One token: its text, its colour (`#rrggbb`, or null for the theme's default) and font style bits (1 italic, 2 bold). */
export type RawToken = [string, string | null, number];

/** `tokens` is null when the file's language is not one of those offered. */
export type SyntaxResponse = { id: number; tokens: RawToken[][][] | null } | { id: number; error: string };

const LANGUAGES: Record<string, () => Promise<{ default: LanguageInput }>> = {
  javascript: () => import("@shikijs/langs/javascript"),
  typescript: () => import("@shikijs/langs/typescript"),
  jsx: () => import("@shikijs/langs/jsx"),
  tsx: () => import("@shikijs/langs/tsx"),
  vue: () => import("@shikijs/langs/vue"),
  svelte: () => import("@shikijs/langs/svelte"),
  html: () => import("@shikijs/langs/html"),
  css: () => import("@shikijs/langs/css"),
  scss: () => import("@shikijs/langs/scss"),
  less: () => import("@shikijs/langs/less"),
  json: () => import("@shikijs/langs/json"),
  jsonc: () => import("@shikijs/langs/jsonc"),
  json5: () => import("@shikijs/langs/json5"),
  yaml: () => import("@shikijs/langs/yaml"),
  toml: () => import("@shikijs/langs/toml"),
  xml: () => import("@shikijs/langs/xml"),
  markdown: () => import("@shikijs/langs/markdown"),
  php: () => import("@shikijs/langs/php"),
  blade: () => import("@shikijs/langs/blade"),
  twig: () => import("@shikijs/langs/twig"),
  python: () => import("@shikijs/langs/python"),
  ruby: () => import("@shikijs/langs/ruby"),
  rust: () => import("@shikijs/langs/rust"),
  go: () => import("@shikijs/langs/go"),
  java: () => import("@shikijs/langs/java"),
  kotlin: () => import("@shikijs/langs/kotlin"),
  scala: () => import("@shikijs/langs/scala"),
  groovy: () => import("@shikijs/langs/groovy"),
  c: () => import("@shikijs/langs/c"),
  cpp: () => import("@shikijs/langs/cpp"),
  "objective-c": () => import("@shikijs/langs/objective-c"),
  csharp: () => import("@shikijs/langs/csharp"),
  fsharp: () => import("@shikijs/langs/fsharp"),
  vb: () => import("@shikijs/langs/vb"),
  swift: () => import("@shikijs/langs/swift"),
  dart: () => import("@shikijs/langs/dart"),
  shellscript: () => import("@shikijs/langs/shellscript"),
  powershell: () => import("@shikijs/langs/powershell"),
  bat: () => import("@shikijs/langs/bat"),
  sql: () => import("@shikijs/langs/sql"),
  docker: () => import("@shikijs/langs/docker"),
  make: () => import("@shikijs/langs/make"),
  ini: () => import("@shikijs/langs/ini"),
  properties: () => import("@shikijs/langs/properties"),
  dotenv: () => import("@shikijs/langs/dotenv"),
  nginx: () => import("@shikijs/langs/nginx"),
  diff: () => import("@shikijs/langs/diff"),
  graphql: () => import("@shikijs/langs/graphql"),
  proto: () => import("@shikijs/langs/proto"),
  hcl: () => import("@shikijs/langs/hcl"),
  terraform: () => import("@shikijs/langs/terraform"),
  lua: () => import("@shikijs/langs/lua"),
  perl: () => import("@shikijs/langs/perl"),
  r: () => import("@shikijs/langs/r"),
  haskell: () => import("@shikijs/langs/haskell"),
  elixir: () => import("@shikijs/langs/elixir"),
  erlang: () => import("@shikijs/langs/erlang"),
  clojure: () => import("@shikijs/langs/clojure"),
  zig: () => import("@shikijs/langs/zig"),
  nix: () => import("@shikijs/langs/nix"),
  latex: () => import("@shikijs/langs/latex"),
};

const THEMES: Record<SyntaxTheme, () => Promise<{ default: ThemeInput }>> = {
  "github-dark-default": () => import("@shikijs/themes/github-dark-default"),
  "github-light-default": () => import("@shikijs/themes/github-light-default"),
};

/** File extensions and whole file names (lower case) to the language above. */
const BY_NAME: Record<string, string> = {
  js: "javascript", mjs: "javascript", cjs: "javascript",
  ts: "typescript", mts: "typescript", cts: "typescript",
  jsx: "jsx", tsx: "tsx", vue: "vue", svelte: "svelte",
  html: "html", htm: "html", css: "css", scss: "scss", less: "less",
  json: "json", jsonc: "jsonc", json5: "json5", yaml: "yaml", yml: "yaml", toml: "toml",
  xml: "xml", svg: "xml", xaml: "xml", csproj: "xml", vbproj: "xml", props: "xml", targets: "xml", plist: "xml",
  md: "markdown", markdown: "markdown",
  php: "php", "blade.php": "blade", twig: "twig",
  py: "python", pyw: "python", rb: "ruby", gemfile: "ruby", rakefile: "ruby",
  rs: "rust", go: "go", java: "java", kt: "kotlin", kts: "kotlin", scala: "scala", groovy: "groovy", gradle: "groovy",
  c: "c", h: "c", cpp: "cpp", cc: "cpp", cxx: "cpp", hpp: "cpp", hh: "cpp", m: "objective-c", mm: "objective-c",
  cs: "csharp", fs: "fsharp", fsx: "fsharp", vb: "vb", swift: "swift", dart: "dart",
  sh: "shellscript", bash: "shellscript", zsh: "shellscript", ps1: "powershell", psm1: "powershell", bat: "bat", cmd: "bat",
  sql: "sql", dockerfile: "docker", makefile: "make", mk: "make",
  ini: "ini", cfg: "ini", properties: "properties", env: "dotenv", conf: "nginx",
  diff: "diff", patch: "diff", graphql: "graphql", gql: "graphql", proto: "proto", hcl: "hcl", tf: "terraform",
  lua: "lua", pl: "perl", pm: "perl", r: "r", hs: "haskell", ex: "elixir", exs: "elixir", erl: "erlang",
  clj: "clojure", cljs: "clojure", zig: "zig", nix: "nix", tex: "latex",
};

/** The language for `path` from its whole file name, a double extension such as `.blade.php`, or its extension. */
function languageFor(path: string): string | null {
  const name = (path.split("/").pop() ?? path).toLowerCase();
  const parts = name.split(".");
  const candidates = [name, parts.slice(-2).join("."), parts[parts.length - 1] ?? ""];
  if (name.startsWith(".env")) candidates.push("env");
  for (const key of candidates) {
    const lang = BY_NAME[key];
    if (lang && LANGUAGES[lang]) return lang;
  }
  return null;
}

let highlighter: Promise<HighlighterCore> | null = null;
const loading = new Map<string, Promise<void>>();

function getHighlighter(): Promise<HighlighterCore> {
  highlighter ??= createHighlighterCore({ themes: [], langs: [], engine: createJavaScriptRegexEngine() });
  return highlighter;
}

/** Loads a language or theme once, however many requests ask for it meanwhile. */
function once(key: string, load: () => Promise<void>): Promise<void> {
  let p = loading.get(key);
  if (!p) {
    p = load();
    loading.set(key, p);
  }
  return p;
}

self.onmessage = async (event: MessageEvent<SyntaxRequest>) => {
  const { id, path, theme, runs } = event.data;
  const lang = languageFor(path);
  if (!lang) {
    self.postMessage({ id, tokens: null } satisfies SyntaxResponse);
    return;
  }
  try {
    const h = await getHighlighter();
    await Promise.all([
      once(`lang:${lang}`, async () => h.loadLanguage((await (LANGUAGES[lang] as () => Promise<{ default: LanguageInput }>)()).default)),
      once(`theme:${theme}`, async () => h.loadTheme((await THEMES[theme]()).default)),
    ]);
    const tokens = runs.map((lines) =>
      h.codeToTokensBase(lines.join("\n"), { lang, theme }).map((line) => line.map((t): RawToken => [t.content, t.color ?? null, t.fontStyle ?? 0])),
    );
    self.postMessage({ id, tokens } satisfies SyntaxResponse);
  } catch (e) {
    self.postMessage({ id, error: e instanceof Error ? e.message : String(e) } satisfies SyntaxResponse);
  }
};
