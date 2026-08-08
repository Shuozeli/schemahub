const HTTP_PROTOCOLS = new Set(["http:", "https:"]);
const WEBSOCKET_PROTOCOLS = new Set(["ws:", "wss:"]);

export async function resolveRemoteCdpEndpoint(endpoint, fetchImpl = fetch) {
  const endpointUrl = new URL(endpoint);
  if (WEBSOCKET_PROTOCOLS.has(endpointUrl.protocol)) {
    return endpointUrl.toString();
  }
  if (!HTTP_PROTOCOLS.has(endpointUrl.protocol)) {
    throw new Error(
      `PLAYWRIGHT_CDP_ENDPOINT must use http, https, ws, or wss: ${endpoint}`,
    );
  }

  const discoveryUrl = `${endpoint.replace(/\/+$/, "")}/json/version`;
  const response = await fetchImpl(discoveryUrl, {
    signal: AbortSignal.timeout(10_000),
  });
  if (!response.ok) {
    throw new Error(
      `CDP discovery failed with HTTP ${response.status}: ${discoveryUrl}`,
    );
  }
  const discovery = await response.json();
  if (
    !discovery ||
    typeof discovery.webSocketDebuggerUrl !== "string" ||
    discovery.webSocketDebuggerUrl.length === 0
  ) {
    throw new Error(`CDP discovery omitted webSocketDebuggerUrl: ${discoveryUrl}`);
  }

  const websocketUrl = new URL(discovery.webSocketDebuggerUrl);
  websocketUrl.protocol = endpointUrl.protocol === "https:" ? "wss:" : "ws:";
  websocketUrl.host = endpointUrl.host;
  return websocketUrl.toString();
}
