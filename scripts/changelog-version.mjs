// Turns the "## Unreleased" section of CHANGELOG.md into the version in package.json, dated
// today, with a new empty "## Unreleased" above it for the commits that follow. `npm version`
// runs it (the "version" script in package.json) after changing package.json and before it
// commits and tags, so the changelog goes into the version's commit. CHANGELOG_FILE names another
// file to change instead, for trying it out.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const { version } = JSON.parse(readFileSync(`${root}package.json`, "utf8"));
const file = process.env.CHANGELOG_FILE ?? `${root}CHANGELOG.md`;

const text = readFileSync(file, "utf8");
if (new RegExp(`^## ${version.replace(/\./g, "\\.")}\\b`, "m").test(text)) {
  throw new Error(`CHANGELOG.md already has a section for ${version}`);
}
if (!/^## Unreleased[ \t]*$/m.test(text)) throw new Error("CHANGELOG.md has no ## Unreleased section");

// Today in local time, as the other sections are dated: 2026-10-28.
const now = new Date();
const date = [now.getFullYear(), now.getMonth() + 1, now.getDate()].map((n) => String(n).padStart(2, "0")).join("-");

const section = text.slice(text.search(/^## Unreleased[ \t]*$/m)).split(/^## (?!Unreleased)/m)[0];
if (!/^- /m.test(section)) console.warn(`CHANGELOG.md: ## Unreleased has no lines; ${version} gets an empty section`);

writeFileSync(file, text.replace(/^## Unreleased[ \t]*$/m, `## Unreleased\n\n## ${version} (${date})`));
console.log(`CHANGELOG.md: ## Unreleased is now ## ${version} (${date})`);
