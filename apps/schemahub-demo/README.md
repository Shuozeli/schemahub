<!-- agent-updated: 2026-07-30T04:16:42Z -->
# SchemaHub Workflow Lab

This Next.js site is the interactive companion to the SchemaHub human–agent
workflow codelab. It simulates the real CLI contract while explaining the
boundary between versioned schema storage and application-data storage.

The guided workflow covers the first scenario in the real-world validation
portfolio:

1. A delegated agent records and validates schema-change intent.
2. A human reviews the exact validated snapshot.
3. The agent applies the approved change with an idempotency key.
4. A data consumer resolves an immutable revision, fetches an artifact, and
   persists its revision and digest beside application data.

The scenario index also links seven executable real-server codelabs: governed
human/agent collaboration, Protobuf commerce rollout, FlatBuffers telemetry
evolution, concurrent editing, a two-repository producer/consumer handoff,
Protobuf dependency-closure evolution, and private-tenant isolation.

The demo never sends production data and does not replace those runners. See
`../../docs/real-world-validation.md` for the complete evidence, severity, and
GA-readiness contract, or run them all with
`../../codelabs/real-world/run-all.sh`.

## Develop

Install the pinned packages:

```bash
pnpm install --frozen-lockfile
```

Bind Next.js to this machine's Tailscale interface:

```bash
export TAILSCALE_IP=$(tailscale ip -4)
export TAILSCALE_HOST=$(tailscale status --json | jq -r '.Self.DNSName' | sed 's/\.$//')
pnpm dev --port 4178
```

Open `http://$TAILSCALE_HOST:4178` from a device on the same tailnet.

## Verify

```bash
pnpm typecheck
pnpm build
pnpm build:sites
pnpm test:worker
SCHEMAHUB_DEMO_URL="http://$TAILSCALE_HOST:4178" pnpm test:browser
```

The Worker smoke starts the static Sites bundle in the real local workerd
runtime and rejects boot/runtime errors. The browser smoke connects to the
neutral Ubuntu GUI's Playwright-compatible CDP listener at
`http://ubuntu-gui-browser-arm2.tail8f3b66.ts.net:9223` by default; set
`PLAYWRIGHT_CDP_ENDPOINT` to override it. Shared discovery rewrites Chrome's
advertised loopback WebSocket onto that Tailscale host and accepts direct
`ws:`/`wss:` endpoints as well. Interactive Pwright sessions use the same
neutral listener from an isolated working directory. The smoke checks all seven runnable
codelab links, walks the lifecycle, switches to FlatBuffers, checks the mobile
layout, and writes screenshots under `/tmp`.

## Deploy

`.openai/hosting.json` binds this source tree to its existing OpenAI Sites
project. Build the static Next.js export and assets-only Worker, push this exact
source state to the Sites source repository, save a version from that commit,
and deploy only the saved version.
