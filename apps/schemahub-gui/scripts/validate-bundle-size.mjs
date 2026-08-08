import assert from "node:assert/strict";
import { readdir, readFile, stat } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const distRoot = path.resolve(
  process.env.SCHEMAHUB_GUI_DIST_DIR ?? path.join(appRoot, "dist"),
);
const maximumEntryBytes = Number.parseInt(
  process.env.SCHEMAHUB_GUI_MAX_ENTRY_BYTES ?? "450000",
  10,
);
const forbiddenRemoteAssetHosts = [
  "cdn.jsdelivr.net",
  "cdnjs.cloudflare.com",
  "cdn.skypack.dev",
  "esm.sh",
  "fonts.googleapis.com",
  "fonts.gstatic.com",
  "unpkg.com",
];

async function collectTextAssets(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];

  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await collectTextAssets(entryPath)));
    } else if (
      entry.isFile() &&
      [".css", ".html", ".js", ".mjs"].includes(path.extname(entry.name))
    ) {
      files.push(entryPath);
    }
  }

  return files;
}

// Arrange
assert(
  Number.isSafeInteger(maximumEntryBytes) && maximumEntryBytes > 0,
  "SCHEMAHUB_GUI_MAX_ENTRY_BYTES must be a positive integer",
);
const indexHtml = await readFile(path.join(distRoot, "index.html"), "utf8");

// Act
const entryMatch = indexHtml.match(
  /<script\b[^>]*\btype="module"[^>]*\bsrc="([^"]+\.js)"[^>]*><\/script>/,
);

// Assert
assert(entryMatch, "production index.html must contain a JavaScript module entry");
const entryRelativePath = entryMatch[1].replace(/^\/+/, "");
const entryPath = path.resolve(distRoot, entryRelativePath);
const relativeToDist = path.relative(distRoot, entryPath);
assert(
  relativeToDist !== "" &&
    relativeToDist !== ".." &&
    !relativeToDist.startsWith(`..${path.sep}`) &&
    !path.isAbsolute(relativeToDist),
  `production entry must remain inside dist: ${entryMatch[1]}`,
);
const entryStat = await stat(entryPath);
assert(entryStat.isFile(), `production entry is not a file: ${entryRelativePath}`);
assert(
  entryStat.size <= maximumEntryBytes,
  `production entry ${entryRelativePath} is ${entryStat.size} bytes; budget is ${maximumEntryBytes} bytes`,
);
const textAssets = await collectTextAssets(distRoot);
for (const assetPath of textAssets) {
  const contents = await readFile(assetPath, "utf8");
  for (const host of forbiddenRemoteAssetHosts) {
    assert(
      !contents.includes(`://${host}/`),
      `production asset ${path.relative(distRoot, assetPath)} depends on remote CDN ${host}`,
    );
  }
}

process.stdout.write(
  `GUI entry bundle is ${entryStat.size} bytes (budget ${maximumEntryBytes} bytes) and has no forbidden runtime CDN references.\n`,
);
