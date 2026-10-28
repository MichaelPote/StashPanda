// Prints the lines CHANGELOG.md lists under one version, for the GitHub release's notes:
// `node scripts/release-notes.mjs 0.6.4`. The section is the one `npm version` made from
// "## Unreleased" (see changelog-version.mjs), headed "## 0.6.4 (2026-10-28)". Exits with 1, and
// prints nothing, when there is no such section or it has no lines, so the release job can fall
// back to GitHub's own notes.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const version = process.argv[2]?.replace(/^v/, "");
if (!version) {
  console.error("Usage: node scripts/release-notes.mjs <version>");
  process.exit(2);
}
const file = process.env.CHANGELOG_FILE ?? fileURLToPath(new URL("../CHANGELOG.md", import.meta.url));
const lines = readFileSync(file, "utf8").split(/\r?\n/);

const heading = new RegExp(`^## ${version.replace(/\./g, "\\.")}(\\s|$)`);
const start = lines.findIndex((l) => heading.test(l));
if (start < 0) {
  console.error(`CHANGELOG.md has no section for ${version}.`);
  process.exit(1);
}
const end = lines.findIndex((l, i) => i > start && l.startsWith("## "));
const body = lines.slice(start + 1, end < 0 ? undefined : end).join("\n").trim();
if (!/^- /m.test(body)) {
  console.error(`CHANGELOG.md lists nothing under ${version}.`);
  process.exit(1);
}
console.log(body);
