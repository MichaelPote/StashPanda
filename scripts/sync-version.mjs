// Copies the version in package.json to the Tauri app: src-tauri/tauri.conf.json, the [package]
// version in src-tauri/Cargo.toml, and the app's own entry in src-tauri/Cargo.lock. `npm version`
// runs it (the "version" script in package.json) after changing package.json and before it
// commits and tags, so all five files go into the one commit. Each file is edited as text, so
// its formatting stays as it was.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const { name, version } = JSON.parse(readFileSync(`${root}package.json`, "utf8"));

/** Replaces the first match of `pattern` in `file`, keeping the text around the version. */
function setVersion(file, pattern) {
  const path = `${root}${file}`;
  const text = readFileSync(path, "utf8");
  if (!pattern.test(text)) throw new Error(`No version found in ${file}`);
  writeFileSync(path, text.replace(pattern, (_, before, after) => `${before}${version}${after}`));
  console.log(`${file}: ${version}`);
}

setVersion("src-tauri/tauri.conf.json", /^(\s*"version":\s*")[^"]*(")/m);
setVersion("src-tauri/Cargo.toml", /^(\[package\][^[]*?\nversion\s*=\s*")[^"]*(")/m);
setVersion("src-tauri/Cargo.lock", new RegExp(`^(name = "${name}"\\nversion = ")[^"]*(")`, "m"));
