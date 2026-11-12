// Writes `latest.json`, the file the app's updater reads from the newest GitHub release.
//
//   node scripts/make-latest-json.mjs <installer.exe> <latest.json> [--tag v0.2.0] [--notes "text"]
//
// The installer's signature (`<installer>.sig`, made by `tauri build` from the signing key) must sit next to it.
// With `--tag` the build fails unless the tag matches the version in tauri.conf.json, so a release can never
// announce a version the installer does not carry. The download address comes from GITHUB_REPOSITORY.
import { readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args.splice(i, 2)[1];
};
const tag = flag("--tag");
const notes = flag("--notes");
const [installer, output] = args;
if (!installer || !output) {
  console.error("Utilizare: node scripts/make-latest-json.mjs <installer.exe> <latest.json> [--tag vX.Y.Z] [--notes text]");
  process.exit(2);
}

const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const version = config.version;
if (tag && tag !== `v${version}`) {
  console.error(`Eticheta ${tag} nu se potrivește cu versiunea din tauri.conf.json (${version}). Schimbă versiunea sau eticheta.`);
  process.exit(1);
}

const repo = process.env.GITHUB_REPOSITORY;
if (!repo) {
  console.error("Lipsește GITHUB_REPOSITORY (owner/repo).");
  process.exit(1);
}

const latest = {
  version,
  notes: notes ?? `Pontaj ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature: readFileSync(`${installer}.sig`, "utf8").trim(),
      url: `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(basename(installer))}`,
    },
  },
};
writeFileSync(output, `${JSON.stringify(latest, null, 2)}\n`);
console.log(`${output}: versiunea ${version} → ${latest.platforms["windows-x86_64"].url}`);
