<!-- agent-updated: 2026-07-23T14:57:16Z -->
# RW-04: Concurrent Human and Agent Editors

This runner gives a human and delegated agent the same immutable base. It
rejects a stale ChangeRecord ETag, publishes both edits to an unprotected
bookmark, renders the resulting first-class declaration conflict, resolves it
explicitly, compiles the merged binding, and restarts the server.

```bash
./codelabs/real-world/rw-04-concurrent-editors/run.sh
```

Expected negative state is an `ABORTED` stale-ETag error. Expected concurrent
state is an applied ChangeRecord whose receipt names `OrderRecord` as
conflicted. Evidence retains both actor records, retry receipts, rendered
conflict, resolution, generated binding, restart checks, and `result.json`.

Follow the guided version in
[`docs/codelab-concurrent-human-agent.md`](../../../docs/codelab-concurrent-human-agent.md).
