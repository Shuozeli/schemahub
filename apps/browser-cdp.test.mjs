import assert from "node:assert/strict";
import test from "node:test";

import { resolveRemoteCdpEndpoint } from "./browser-cdp.mjs";

test("a direct WebSocket CDP endpoint does not require discovery", async () => {
  // Arrange
  const directEndpoint = "ws://browser.example.test:9223/devtools/browser/session";
  const unexpectedFetch = async () => {
    throw new Error("direct WebSocket endpoints must not use HTTP discovery");
  };

  // Act
  const resolved = await resolveRemoteCdpEndpoint(directEndpoint, unexpectedFetch);

  // Assert
  assert.equal(resolved, directEndpoint);
});

test("HTTP discovery rewrites a loopback WebSocket onto the remote CDP host", async () => {
  // Arrange
  const requestedUrls = [];
  const discoveryFetch = async (url) => {
    requestedUrls.push(url);
    return {
      ok: true,
      async json() {
        return {
          webSocketDebuggerUrl: "ws://127.0.0.1/devtools/browser/session",
        };
      },
    };
  };

  // Act
  const resolved = await resolveRemoteCdpEndpoint(
    "http://browser.example.test:9223",
    discoveryFetch,
  );

  // Assert
  assert.deepEqual(requestedUrls, [
    "http://browser.example.test:9223/json/version",
  ]);
  assert.equal(
    resolved,
    "ws://browser.example.test:9223/devtools/browser/session",
  );
});

test("HTTPS discovery upgrades the rewritten WebSocket transport", async () => {
  // Arrange
  const discoveryFetch = async () => ({
    ok: true,
    async json() {
      return {
        webSocketDebuggerUrl: "ws://localhost/devtools/browser/session",
      };
    },
  });

  // Act
  const resolved = await resolveRemoteCdpEndpoint(
    "https://browser.example.test",
    discoveryFetch,
  );

  // Assert
  assert.equal(resolved, "wss://browser.example.test/devtools/browser/session");
});

test("discovery fails closed when Chrome omits its WebSocket URL", async () => {
  // Arrange
  const discoveryFetch = async () => ({
    ok: true,
    async json() {
      return {};
    },
  });

  // Act
  const resolution = resolveRemoteCdpEndpoint(
    "http://browser.example.test:9223",
    discoveryFetch,
  );

  // Assert
  await assert.rejects(resolution, /omitted webSocketDebuggerUrl/);
});
