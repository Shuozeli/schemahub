import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptsRoot = path.dirname(fileURLToPath(import.meta.url));
const validator = path.join(scriptsRoot, "validate-bundle-size.mjs");
const fixtureRoot = await mkdtemp(
  path.join(tmpdir(), "schemahub-gui-bundle-contract-"),
);
const assetsRoot = path.join(fixtureRoot, "assets");

function validate(extraEnvironment = {}) {
  return spawnSync(process.execPath, [validator], {
    encoding: "utf8",
    env: {
      ...process.env,
      SCHEMAHUB_GUI_DIST_DIR: fixtureRoot,
      ...extraEnvironment,
    },
  });
}

try {
  // Arrange
  await mkdir(assetsRoot);
  await writeFile(
    path.join(fixtureRoot, "index.html"),
    '<script type="module" src="/assets/index.js"></script>\n',
  );
  await writeFile(path.join(assetsRoot, "index.js"), "export const ready = true;\n");

  // Act
  const validResult = validate();

  // Assert
  assert.equal(validResult.status, 0, validResult.stderr);
  assert.match(validResult.stdout, /no forbidden runtime CDN references/);

  // Arrange
  await writeFile(
    path.join(assetsRoot, "index.js"),
    'export const loader = "https://cdn.jsdelivr.net/runtime.js";\n',
  );

  // Act
  const cdnResult = validate();

  // Assert
  assert.notEqual(cdnResult.status, 0);
  assert.match(cdnResult.stderr, /depends on remote CDN cdn\.jsdelivr\.net/);

  // Arrange
  await writeFile(path.join(assetsRoot, "index.js"), "export const ready = true;\n");

  // Act
  const oversizedResult = validate({
    SCHEMAHUB_GUI_MAX_ENTRY_BYTES: "1",
  });

  // Assert
  assert.notEqual(oversizedResult.status, 0);
  assert.match(oversizedResult.stderr, /budget is 1 bytes/);
} finally {
  await rm(fixtureRoot, { recursive: true, force: true });
}

process.stdout.write("GUI bundle validation contract tests passed.\n");
