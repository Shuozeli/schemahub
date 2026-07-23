export type Actor = "agent" | "human" | "consumer";
export type SchemaFormat = "protobuf" | "flatbuffers";
export type WorkflowStatus =
  | "draft"
  | "validated"
  | "ready"
  | "approved"
  | "applied"
  | "served";

export interface FormatExample {
  label: string;
  shortLabel: string;
  schemaPath: string;
  schemaSource: string;
  generatedSymbol: string;
  descriptorLabel: string;
}

export interface WorkflowStep {
  actor: Actor;
  command: (format: FormatExample) => string;
  detail: string;
  eyebrow: string;
  id: string;
  output: (format: FormatExample) => string;
  status: WorkflowStatus;
  title: string;
}

export const formats: Record<SchemaFormat, FormatExample> = {
  protobuf: {
    label: "Protobuf order record",
    shortLabel: "Protobuf",
    schemaPath: "orders/v1/order.proto",
    generatedSymbol: "OrderRecord",
    descriptorLabel: "FileDescriptorSet",
    schemaSource: `syntax = "proto3";
package codelab.orders.v1;

message OrderRecord {
  string id = 1;
  int64 created_at_unix_ms = 2;
  bytes payload = 3;
}`,
  },
  flatbuffers: {
    label: "FlatBuffers event record",
    shortLabel: "FlatBuffers",
    schemaPath: "events/v1/event.fbs",
    generatedSymbol: "EventRecord",
    descriptorLabel: "reconstructed .fbs bundle",
    schemaSource: `namespace codelab.events.v1;

table EventRecord {
  id: string;
  created_at_unix_ms: long;
  payload: [ubyte];
}

root_type EventRecord;`,
  },
};

export const steps: readonly WorkflowStep[] = [
  {
    id: "record",
    actor: "agent",
    eyebrow: "01 / Intent",
    title: "Record the reason",
    status: "draft",
    detail:
      "The agent creates a durable ChangeRecord before touching schema state. Its delegated identity and external reference become part of the audit trail.",
    command: () => `schemahub change note codelab/registry \\
  --title "Introduce a persisted record envelope" \\
  --reference CODELAB-1 \\
  --id introduce-record --json`,
    output: () => `{
  "status": "draft",
  "created_by": {
    "identity": "schema-agent",
    "kind": "agent",
    "delegated_by": "human-owner"
  },
  "etag": "v1"
}`,
  },
  {
    id: "validate",
    actor: "agent",
    eyebrow: "02 / Compile",
    title: "Attach and validate",
    status: "validated",
    detail:
      "SchemaHub parses the proposed source, resolves the exact base, checks compatibility and references, and stores the validation snapshot on the record.",
    command: (format) => `schemahub change add-source "$CHANGE" \\
  --etag v1 \\
  --schema-path ${format.schemaPath} \\
  --file ${format.schemaPath.split("/").at(-1)} --json

schemahub change validate "$CHANGE" \\
  --etag v2 --json`,
    output: () => `{
  "validation": {
    "valid": true,
    "resolved_base_commit": "9d0f…bf42",
    "edit_digest": "sha256:3e8b…91ac",
    "issues": []
  },
  "etag": "v3"
}`,
  },
  {
    id: "ready",
    actor: "agent",
    eyebrow: "03 / Handoff",
    title: "Mark the snapshot ready",
    status: "ready",
    detail:
      "Ready freezes the relationship between the edit set and its passing validation. A changed edit would clear validation and return the record to Draft.",
    command: () => `schemahub change ready "$CHANGE" \\
  --etag v3 --json`,
    output: () => `{
  "status": "ready",
  "validation": { "valid": true },
  "reviews": [],
  "etag": "v4"
}`,
  },
  {
    id: "approve",
    actor: "human",
    eyebrow: "04 / Gate",
    title: "Human reviews the exact change",
    status: "approved",
    detail:
      "The repository requires one Maintainer-or-Owner approval. The author cannot review their own change, and the reviewer comes from the bearer token.",
    command: () => `schemahub change approve "$CHANGE" \\
  --etag v4 \\
  --reason "Wire contract reviewed" --json`,
    output: () => `{
  "status": "ready",
  "reviews": [{
    "decision": "approved",
    "reviewer": {
      "identity": "human-owner",
      "kind": "human"
    }
  }],
  "etag": "v5"
}`,
  },
  {
    id: "apply",
    actor: "agent",
    eyebrow: "05 / Publish",
    title: "Apply exactly once",
    status: "applied",
    detail:
      "The agent reuses one request ID for retries. SchemaHub links the durable ChangeRecord to the resulting JJ commit and operation receipt.",
    command: () => `schemahub change apply "$CHANGE" \\
  --etag v5 \\
  --request-id apply-introduce-record --json`,
    output: () => `{
  "status": "applied",
  "apply_result": {
    "commit_id": "ab0c5d…ab7ac",
    "operation_id": "cab737…fdd3e",
    "conflicted_declarations": []
  }
}`,
  },
  {
    id: "serve",
    actor: "consumer",
    eyebrow: "06 / Consume",
    title: "Pin and verify the artifact",
    status: "served",
    detail:
      "The consumer resolves main once, then fetches through the immutable revision. It stores the revision and digest beside application data for future decoding.",
    command: (format) => `schemahub artifact resolve codelab/registry \\
  --at main --json

schemahub artifact fetch "$REVISION" \\
  --schema-path ${format.schemaPath} \\
  --kind descriptors \\
  --output schema.desc --json`,
    output: (format) => `{
  "revision": "projects/codelab/repos/registry/revisions/ab0c5d…ab7ac",
  "schema_path": "${format.schemaPath}",
  "kind": "descriptors",
  "artifact_digest": "sha256:a5bd…0f52f",
  "closure_digest": "sha256:16e1…d6c0"
}`,
  },
] as const;

export const actorLabels: Record<Actor, string> = {
  agent: "Delegated agent",
  human: "Human owner",
  consumer: "Data consumer",
};

export const statusLabels: Record<WorkflowStatus, string> = {
  draft: "Draft",
  validated: "Validated",
  ready: "Ready",
  approved: "Approved",
  applied: "Applied",
  served: "Artifact served",
};
