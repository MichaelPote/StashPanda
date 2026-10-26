// Writes latest.json, which the app's updater reads from the newest published GitHub Release
// (plugins.updater in src-tauri/tauri.conf.json). Run by the release job in
// .github/workflows/build.yml, with the folder of downloaded installers and their .sig files:
//
//   node scripts/updater-manifest.mjs <folder> <tag> <output file>
//
// Each installer that has a .sig beside it gets an entry, under the name the updater looks for:
// `{os}-{arch}-{package type}` first, then `{os}-{arch}` for a build that does not know how it
// was installed, which gets the Windows installer or the AppImage.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";

const [folder, tag, output] = process.argv.slice(2);
if (!folder || !tag || !output) {
  console.error("Usage: node scripts/updater-manifest.mjs <folder> <tag> <output file>");
  process.exit(1);
}

const REPO_URL = "https://github.com/MichaelPote/StashPanda";

/** Installer file name endings, and the updater platform names each is listed under. */
const KINDS = [
  { suffix: "-setup.exe", platforms: ["windows-x86_64-nsis", "windows-x86_64"] },
  { suffix: ".AppImage", platforms: ["linux-x86_64-appimage", "linux-x86_64"] },
  { suffix: ".deb", platforms: ["linux-x86_64-deb"] },
  { suffix: ".rpm", platforms: ["linux-x86_64-rpm"] },
];

/** Every file under `dir`, at any depth. */
function listFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? listFiles(join(dir, e.name)) : [join(dir, e.name)],
  );
}

const files = listFiles(folder);
const platforms = {};
for (const file of files) {
  const kind = KINDS.find((k) => file.endsWith(k.suffix));
  if (!kind) continue;
  const sigFile = `${file}.sig`;
  if (!files.includes(sigFile)) {
    console.error(`${basename(file)} has no .sig file, so it was not signed.`);
    process.exit(1);
  }
  // The release keeps each file under its own name, so this is where the updater downloads it.
  const url = `${REPO_URL}/releases/download/${tag}/${encodeURIComponent(basename(file))}`;
  const signature = readFileSync(sigFile, "utf8").trim();
  for (const platform of kind.platforms) platforms[platform] = { url, signature };
}

for (const required of ["windows-x86_64", "linux-x86_64"]) {
  if (!platforms[required]) {
    console.error(`No installer was found for ${required}.`);
    process.exit(1);
  }
}

const manifest = {
  version: tag.replace(/^v/, ""),
  // The release notes are edited on the Releases page after this runs, so the app links there
  // instead of showing a copy that would go stale.
  notes: "",
  pub_date: new Date().toISOString(),
  platforms,
};
writeFileSync(output, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`Wrote ${output} for ${Object.keys(platforms).join(", ")}`);
