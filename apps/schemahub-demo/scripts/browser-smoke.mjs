import assert from "node:assert/strict";

import { chromium } from "playwright-core";

const demoUrl = process.env.SCHEMAHUB_DEMO_URL;
const localChromium = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE;

if (!demoUrl) {
  throw new Error(
    "SCHEMAHUB_DEMO_URL is required; use the full Tailscale MagicDNS URL.",
  );
}

const browser = localChromium
  ? await chromium.launch({
      executablePath: localChromium,
      headless: true,
    })
  : await chromium.connectOverCDP("http://10.0.0.149:9000");
const context = browser.contexts()[0] ?? (await browser.newContext());
const page = await context.newPage();
const consoleErrors = [];

page.on("console", (message) => {
  if (message.type() === "error") {
    consoleErrors.push(message.text());
  }
});

try {
  // Arrange
  await page.setViewportSize({ width: 1440, height: 1000 });

  // Act
  const response = await page.goto(demoUrl, {
    waitUntil: "networkidle",
    timeout: 30_000,
  });

  // Assert
  assert(response, "the demo navigation should return a response");
  assert.equal(response.ok(), true, `the demo should load successfully: ${response.status()}`);
  await page.getByRole("heading", { level: 1 }).waitFor();
  assert.match(await page.title(), /SchemaHub Workflow Lab/);
  assert.match(
    await page.getByRole("heading", { level: 1 }).innerText(),
    /Schema changes are/,
  );
  assert.equal(
    await page.locator('[data-testid="scenario-portfolio"] article').count(),
    5,
  );
  await page.getByText("1 scenario running", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Run this step" }).click();
  await page.getByText("schemahub.change.draft", { exact: true }).waitFor();
  await page.getByRole("button", { name: "Continue to step 2" }).click();
  await page.getByRole("heading", { name: "Attach and validate" }).waitFor();
  await page.screenshot({
    fullPage: true,
    path: "/tmp/schemahub-demo-desktop.png",
  });

  // Arrange
  await page.getByRole("button", { name: /FlatBuffers/ }).click();

  // Act
  const schemaSource = await page.locator(".source-card pre").innerText();

  // Assert
  assert.match(schemaSource, /table EventRecord/);
  assert.match(
    await page.locator(".source-card").innerText(),
    /events\/v1\/event\.fbs/i,
  );
  assert.equal(
    await page.getByText("Run step 1 to record the first durable event.").isVisible(),
    true,
  );

  // Arrange
  await page.setViewportSize({ width: 390, height: 844 });

  // Act
  await page.reload({ waitUntil: "networkidle" });

  // Assert
  await page.getByRole("heading", { level: 1 }).waitFor();
  const horizontalOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  );
  assert(
    horizontalOverflow <= 1,
    `mobile layout should not overflow horizontally (overflow=${horizontalOverflow}px)`,
  );
  await page.screenshot({
    fullPage: true,
    path: "/tmp/schemahub-demo-mobile.png",
  });
  assert.deepEqual(consoleErrors, [], `browser console errors: ${consoleErrors.join("\n")}`);

  process.stdout.write(
    "Browser smoke passed; desktop and mobile screenshots written to /tmp.\n",
  );
} catch (error) {
  await page.screenshot({
    fullPage: true,
    path: "/tmp/schemahub-demo-failure.png",
  });
  process.stderr.write(
    `Browser smoke failed; console errors: ${consoleErrors.join(" | ") || "none"}\n`,
  );
  throw error;
} finally {
  await page.close();
  await browser.close();
}
