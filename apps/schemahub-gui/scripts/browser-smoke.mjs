import assert from "node:assert/strict";

import { chromium } from "playwright-core";
import { resolveRemoteCdpEndpoint } from "../../browser-cdp.mjs";

const guiUrl = process.env.SCHEMAHUB_GUI_URL;
const localChromium = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;
const remoteCdp =
  process.env.PLAYWRIGHT_CDP_ENDPOINT ??
  "http://ubuntu-gui-browser-arm2.tail8f3b66.ts.net:9223";

if (!guiUrl) {
  throw new Error(
    "SCHEMAHUB_GUI_URL is required; use the full Tailscale MagicDNS URL outside CI.",
  );
}

const browser = localChromium
  ? await chromium.launch({
      executablePath: localChromium,
      headless: true,
    })
  : await chromium.connectOverCDP(await resolveRemoteCdpEndpoint(remoteCdp));
const context = browser.contexts()[0] ?? (await browser.newContext());
const page = await context.newPage();
const browserErrors = [];

page.on("console", (message) => {
  if (message.type() === "error") {
    browserErrors.push(message.text());
  }
});
page.on("pageerror", (error) => browserErrors.push(error.message));

const initialSource = `syntax = "proto3";
package commerce.browser.v1;

message BrowserOrder {
  string id = 1;
}
`;
const updatedSource = `syntax = "proto3";
package commerce.browser.v1;

message BrowserOrder {
  string id = 1;
  string storage_note = 2;
}
`;

async function readHeaderLayout(page) {
  return page.locator("header").evaluate((header) => {
    const identity = header.querySelector(".identityMenuButton");
    if (!(identity instanceof HTMLElement)) {
      throw new Error("identity control is missing from the operator header");
    }
    const headerRect = header.getBoundingClientRect();
    const identityRect = identity.getBoundingClientRect();
    return {
      clientHeight: header.clientHeight,
      scrollHeight: header.scrollHeight,
      identityTop: identityRect.top,
      identityBottom: identityRect.bottom,
      headerTop: headerRect.top,
      headerBottom: headerRect.bottom,
    };
  });
}

function assertHeaderFits(layout) {
  assert.equal(
    layout.scrollHeight,
    layout.clientHeight,
    "operator header must not wrap beyond its fixed height",
  );
  assert(
    layout.identityTop >= layout.headerTop &&
      layout.identityBottom <= layout.headerBottom,
    `identity control escaped header bounds: ${JSON.stringify(layout)}`,
  );
}

try {
  // Arrange
  await page.setViewportSize({ width: 930, height: 900 });

  // Act
  const projectsResponse = await page.goto(
    `${guiUrl.replace(/\/$/, "")}/projects`,
    {
      waitUntil: "networkidle",
      timeout: 30_000,
    },
  );

  // Assert
  assert(projectsResponse, "project navigation should return a response");
  assert.equal(projectsResponse.ok(), true);
  await page.getByRole("heading", { name: "Projects", exact: true }).waitFor();
  await page.getByRole("cell", { name: "acme", exact: true }).waitFor();
  assert.equal(
    await page.getByRole("cell", { name: "platform", exact: true }).count(),
    0,
    "second project must not arrive before its continuation is requested",
  );

  // Act
  await page.getByRole("button", { name: "Load more projects" }).click();

  // Assert
  await page.getByRole("cell", { name: "platform", exact: true }).waitFor();

  // Act
  await page.goto(`${guiUrl.replace(/\/$/, "")}/projects/acme`, {
    waitUntil: "networkidle",
    timeout: 30_000,
  });

  // Assert
  await page.getByRole("cell", { name: "billing", exact: true }).waitFor();
  assert.equal(
    await page.getByRole("cell", { name: "commerce", exact: true }).count(),
    0,
    "second repository must not arrive before its continuation is requested",
  );

  // Act
  await page.getByRole("button", { name: "Load more repositories" }).click();

  // Assert
  await page.getByRole("cell", { name: "commerce", exact: true }).waitFor();

  // Act
  await page.goto(
    `${guiUrl.replace(/\/$/, "")}/projects/acme/repos/commerce`,
    {
      waitUntil: "networkidle",
      timeout: 30_000,
    },
  );

  // Assert
  await page.getByRole("heading", { name: "Repo dashboard" }).waitFor();
  await page.getByRole("cell", { name: "build_record.fbs", exact: true }).waitFor();
  assert.equal(
    await page.getByRole("cell", { name: "commerce.yaml", exact: true }).count(),
    0,
    "second dashboard schema must wait for its composite continuation",
  );

  // Act
  await page
    .getByRole("button", { name: "Load more schemas and refs" })
    .click();

  // Assert
  await page.getByRole("cell", { name: "commerce.yaml", exact: true }).waitFor();

  // Act
  const response = await page.goto(
    `${guiUrl.replace(/\/$/, "")}/projects/acme/repos/commerce/changes`,
    {
      waitUntil: "networkidle",
      timeout: 30_000,
    },
  );

  // Assert
  assert(response, "GUI navigation should return a response");
  assert.equal(response.ok(), true, `GUI should load successfully: ${response.status()}`);
  await page.getByRole("heading", { name: "Change proposals" }).waitFor();
  assertHeaderFits(await readHeaderLayout(page));

  // Arrange
  await page.setViewportSize({ width: 390, height: 844 });

  // Act
  const mobileHeaderLayout = await readHeaderLayout(page);

  // Assert
  assertHeaderFits(mobileHeaderLayout);
  assert.equal(await page.locator(".globalSearch").isVisible(), false);
  assert.equal(await page.locator(".identityMenuLabel").isVisible(), false);
  await page.setViewportSize({ width: 1440, height: 1100 });

  // Arrange
  await page.getByRole("button", { name: "Create proposal" }).click();
  const createDialog = page.getByRole("dialog");
  await createDialog.getByLabel("Title").fill("Browser-authored order contract");
  await createDialog
    .getByLabel("Description")
    .fill("Exercise executable source authoring through the typed browser client.");
  await createDialog.getByRole("button", { name: "Add executable edit" }).click();
  await createDialog.getByLabel("Schema path").fill("schemas/browser-order.proto");
  await createDialog.getByLabel("Complete schema source").fill(initialSource);

  // Act
  await createDialog
    .getByRole("button", { name: "Create executable proposal" })
    .click();

  // Assert
  await page.getByRole("heading", { name: "Browser-authored order contract" }).waitFor();
  await page
    .getByRole("cell", { name: "schemas/browser-order.proto", exact: true })
    .waitFor();
  await page.getByText(`${initialSource.length} source characters`, { exact: true }).waitFor();

  // Arrange
  await page.getByRole("button", { name: "Validate" }).click();
  await page.getByText("passing", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const editDialog = page.getByRole("dialog");
  await editDialog.getByLabel("Complete schema source").fill(updatedSource);

  // Act
  await editDialog.getByRole("button", { name: "Save executable edits" }).click();

  // Assert
  await editDialog.waitFor({ state: "hidden" });
  await page.getByText(`${updatedSource.length} source characters`, { exact: true }).waitFor();
  await page.getByText("not run", { exact: true }).waitFor();

  // Arrange
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  const expandedDialog = page.getByRole("dialog");
  await expandedDialog.getByRole("button", { name: "Add executable edit" }).click();
  await expandedDialog.getByLabel("Edit kind").nth(1).click();
  await page.getByRole("option", { name: "Delete schema" }).click();
  await expandedDialog
    .getByLabel("Schema path")
    .nth(1)
    .fill("schemas/legacy-order.proto");

  // Act
  await expandedDialog
    .getByRole("button", { name: "Save executable edits" })
    .click();

  // Assert
  await expandedDialog.waitFor({ state: "hidden" });
  await page
    .getByRole("cell", { name: "schemas/legacy-order.proto", exact: true })
    .waitFor();
  await page.getByRole("cell", { name: "deletion", exact: true }).waitFor();
  await page.screenshot({
    fullPage: true,
    path: "/tmp/schemahub-gui-edit-authoring.png",
  });

  // Act
  await page.getByRole("link", { name: "Changes", exact: true }).click();

  // Assert
  await page.getByRole("heading", { name: "Change proposals" }).waitFor();
  assert.equal(
    await page.getByText("Browser-authored order contract", { exact: true }).count(),
    0,
    "newer proposal must wait for the indexed ChangeRecord continuation",
  );

  // Act
  await page.getByRole("button", { name: "Load newer proposals" }).click();

  // Assert
  await page.getByText("Browser-authored order contract", { exact: true }).waitFor();

  // Arrange
  const schemaUrl = `${guiUrl.replace(/\/$/, "")}/projects/acme/repos/commerce/schemas/order.proto`;

  // Act
  await page.goto(schemaUrl, {
    waitUntil: "networkidle",
    timeout: 30_000,
  });
  const codeViewer = page.getByRole("region", { name: "protobuf source code" });
  await codeViewer.waitFor();
  await codeViewer.focus();

  // Assert
  const viewerContract = await codeViewer.evaluate((viewer) => {
    const lines = viewer.querySelectorAll(".codeViewerLine");
    const firstLine = lines.item(0);
    return {
      focused: document.activeElement === viewer,
      lineCount: lines.length,
      firstLine: firstLine.textContent,
      firstLineNumber: window.getComputedStyle(firstLine, "::before").content,
      overflow: window.getComputedStyle(viewer).overflow,
    };
  });
  assert.equal(viewerContract.focused, true);
  assert.equal(viewerContract.lineCount, 11);
  assert.equal(viewerContract.firstLine, 'syntax = "proto3";');
  assert.equal(viewerContract.firstLineNumber, '"1"');
  assert.equal(viewerContract.overflow, "auto");
  const externalResources = await page.evaluate(() =>
    performance
      .getEntriesByType("resource")
      .map((entry) => entry.name)
      .filter((name) => !name.startsWith(window.location.origin)),
  );
  assert.deepEqual(
    externalResources,
    [],
    `GUI loaded third-party runtime resources: ${externalResources.join(", ")}`,
  );
  assert.deepEqual(browserErrors, [], `browser errors: ${browserErrors.join("\n")}`);

  process.stdout.write(
    "GUI edit-authoring browser smoke passed; screenshot written to /tmp/schemahub-gui-edit-authoring.png.\n",
  );
} catch (error) {
  await page.screenshot({
    fullPage: true,
    path: "/tmp/schemahub-gui-edit-authoring-failure.png",
  });
  process.stderr.write(
    `GUI browser smoke failed; browser errors: ${browserErrors.join(" | ") || "none"}\n`,
  );
  throw error;
} finally {
  await page.close();
  await browser.close();
}
