import assert from "node:assert/strict";

import { chromium } from "playwright-core";
import { resolveRemoteCdpEndpoint } from "../../browser-cdp.mjs";

const guiUrl = process.env.SCHEMAHUB_GUI_URL?.replace(/\/$/, "");
const agentToken = process.env.SCHEMAHUB_GUI_AGENT_TOKEN;
const humanToken = process.env.SCHEMAHUB_GUI_HUMAN_TOKEN;
const screenshotPath =
  process.env.SCHEMAHUB_GUI_SCREENSHOT ||
  "/tmp/schemahub-gui-live-browser.png";
const localChromium = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;
const remoteCdp =
  process.env.PLAYWRIGHT_CDP_ENDPOINT ??
  "http://ubuntu-gui-browser-arm2.tail8f3b66.ts.net:9223";

if (!guiUrl || !agentToken || !humanToken) {
  throw new Error(
    "SCHEMAHUB_GUI_URL, SCHEMAHUB_GUI_AGENT_TOKEN, and " +
      "SCHEMAHUB_GUI_HUMAN_TOKEN are required.",
  );
}

const browser = localChromium
  ? await chromium.launch({
      executablePath: localChromium,
      headless: true,
    })
  : await chromium.connectOverCDP(await resolveRemoteCdpEndpoint(remoteCdp));
const context = await browser.newContext();
await context.addInitScript((token) => {
  if (!window.sessionStorage.getItem("schemahub.live-smoke.initialized")) {
    window.localStorage.setItem("schemahub.token", token);
    window.sessionStorage.setItem("schemahub.live-smoke.initialized", "true");
  }
}, agentToken);

const page = await context.newPage();
const browserErrors = [];
const unexpectedServerResponses = [];
const expectedPreconditionResponses = [];

page.on("console", (message) => {
  if (message.type() === "error") {
    browserErrors.push(message.text());
  }
});
page.on("pageerror", (error) => browserErrors.push(error.message));
page.on("response", (response) => {
  const request = response.request();
  if (
    response.status() === 412 &&
    request.method() === "POST" &&
    response.url().endsWith("/actions/apply")
  ) {
    expectedPreconditionResponses.push(
      `${response.status()} ${request.method()} ${response.url()}`,
    );
  }
  if (response.status() >= 500) {
    unexpectedServerResponses.push(
      `${response.status()} ${request.method()} ${response.url()}`,
    );
  }
});

const title = "Live browser governed contract";
const schemaPath = "schemas/live-browser.proto";
const source = `syntax = "proto3";
package gui.live.v1;

message LiveBrowserRecord {
  string id = 1;
  string storage_note = 2;
}
`;

async function switchIdentity(token, displayName) {
  // Arrange
  await page.evaluate((nextToken) => {
    window.localStorage.setItem("schemahub.token", nextToken);
  }, token);

  // Act
  await page.reload({ waitUntil: "networkidle" });

  // Assert
  await page
    .getByRole("button", {
      name: `Identity: ${displayName}`,
      exact: true,
    })
    .waitFor();
  await page.getByRole("heading", { name: title, exact: true }).waitFor();
}

try {
  // Arrange
  await page.setViewportSize({ width: 1440, height: 1100 });

  // Act
  const response = await page.goto(
    `${guiUrl}/projects/gui/repos/contracts/changes`,
    {
      waitUntil: "networkidle",
      timeout: 30_000,
    },
  );

  // Assert
  assert(response, "live GUI navigation should return a response");
  assert.equal(
    response.ok(),
    true,
    `live GUI should load successfully: ${response.status()}`,
  );
  await page
    .getByRole("button", {
      name: "Identity: Delegated GUI Agent",
      exact: true,
    })
    .waitFor();
  await page.getByRole("heading", { name: "Change proposals" }).waitFor();
  await page.getByText("live API", { exact: true }).waitFor();

  // Arrange
  await page.getByRole("button", { name: "Create proposal" }).click();
  const createDialog = page.getByRole("dialog");
  await createDialog.getByLabel("Title").fill(title);
  await createDialog
    .getByLabel("Description")
    .fill(
      "A delegated agent authors source; an independent human approves before Apply.",
    );
  await createDialog
    .getByLabel("External references")
    .fill("GUI-LIVE-ACCEPTANCE-1");
  await createDialog.getByRole("button", { name: "Add executable edit" }).click();
  await createDialog.getByLabel("Schema path").fill(schemaPath);
  await createDialog.getByLabel("Complete schema source").fill(source);

  // Act
  await createDialog
    .getByRole("button", { name: "Create executable proposal" })
    .click();

  // Assert
  await page.getByRole("heading", { name: title, exact: true }).waitFor();
  await page.getByText("Delegated GUI Agent", { exact: true }).first().waitFor();
  await page
    .getByText("agent · delegated by gui-owner", { exact: true })
    .waitFor();
  await page.getByRole("cell", { name: schemaPath, exact: true }).waitFor();

  // Arrange
  await page.getByRole("button", { name: "Validate" }).click();
  await page.getByText("passing", { exact: true }).waitFor();

  // Act
  await page.getByRole("button", { name: "Mark ready" }).click();

  // Assert
  await page.getByRole("button", { name: "Apply safely" }).waitFor();
  await page
    .getByText("No human or maintainer review has been recorded.", {
      exact: true,
    })
    .waitFor();

  // Act
  await page.getByRole("button", { name: "Apply safely" }).click();

  // Assert
  await page
    .getByRole("alert")
    .filter({ hasText: "Action was not accepted" })
    .waitFor();
  assert.equal(
    expectedPreconditionResponses.length,
    1,
    "pre-review Apply should produce exactly one POST 412 response",
  );

  await switchIdentity(humanToken, "GUI Owner");

  // Arrange
  await page
    .getByLabel("Review reason")
    .fill("Reviewed compiler validation and persisted-data compatibility.");

  // Act
  await page.getByRole("button", { name: "Approve", exact: true }).click();

  // Assert
  await page.getByText("approved", { exact: true }).waitFor();
  await page.getByText("GUI Owner", { exact: true }).first().waitFor();

  await switchIdentity(agentToken, "Delegated GUI Agent");

  // Act
  await page.getByRole("button", { name: "Apply safely" }).click();

  // Assert
  await page
    .getByRole("heading", { name: "Immutable apply receipt", exact: true })
    .waitFor();
  await page
    .getByText("Applied without declaration conflicts.", { exact: true })
    .waitFor();

  // Act
  await page.goto(
    `${guiUrl}/projects/gui/repos/contracts/schemas/${schemaPath}?ref=main`,
    { waitUntil: "networkidle" },
  );

  // Assert
  await page.getByRole("heading", { name: schemaPath, exact: true }).waitFor();
  await page.getByText("LiveBrowserRecord", { exact: true }).first().waitFor();
  await page.screenshot({ fullPage: true, path: screenshotPath });
  const expectedPreconditionConsoleErrors = browserErrors.filter((message) =>
    message.includes("412 (Precondition Failed)"),
  );
  const unexpectedBrowserErrors = browserErrors.filter(
    (message) => !message.includes("412 (Precondition Failed)"),
  );
  assert.equal(
    expectedPreconditionConsoleErrors.length,
    1,
    "pre-review Apply should produce exactly one expected 412 console error",
  );
  assert.deepEqual(
    unexpectedBrowserErrors,
    [],
    `unexpected browser errors: ${unexpectedBrowserErrors.join("\n")}`,
  );
  assert.deepEqual(
    unexpectedServerResponses,
    [],
    `unexpected 5xx responses: ${unexpectedServerResponses.join("\n")}`,
  );

  process.stdout.write(
    `Live GUI browser acceptance passed; screenshot written to ${screenshotPath}.\n`,
  );
} catch (error) {
  await page.screenshot({
    fullPage: true,
    path: screenshotPath.replace(/(\.[^.]+)?$/, "-failure$1"),
  });
  process.stderr.write(
    `Live GUI browser acceptance failed; browser errors: ${
      browserErrors.join(" | ") || "none"
    }; 5xx responses: ${unexpectedServerResponses.join(" | ") || "none"}\n`,
  );
  throw error;
} finally {
  await page.close();
  await context.close();
  await browser.close();
}
