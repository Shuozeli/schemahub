"use client";

import { useMemo, useState } from "react";

import {
  actorLabels,
  formats,
  statusLabels,
  steps,
  type Actor,
  type SchemaFormat,
} from "../lib/workflow";

const actorGlyphs: Record<Actor, string> = {
  agent: "A",
  human: "H",
  consumer: "C",
};

function ArrowIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="M5 12h13M14 7l5 5-5 5" />
    </svg>
  );
}

function CheckIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="m5 12 4 4L19 6" />
    </svg>
  );
}

function CopyIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <rect x="8" y="8" width="11" height="11" rx="2" />
      <path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2" />
    </svg>
  );
}

function GridMark() {
  return (
    <svg aria-hidden="true" className="brand-mark" viewBox="0 0 48 48">
      <path d="M8 10h32v9H8zm0 19h32v9H8z" />
      <path className="brand-mark-accent" d="M16 6v36M32 6v36" />
      <circle cx="16" cy="14.5" r="4" />
      <circle className="brand-mark-gold" cx="32" cy="33.5" r="4" />
    </svg>
  );
}

function ActorBadge({ actor }: Readonly<{ actor: Actor }>) {
  return (
    <span className={`actor-badge actor-${actor}`}>
      <span>{actorGlyphs[actor]}</span>
      {actorLabels[actor]}
    </span>
  );
}

function CodePanel({
  copied,
  label,
  onCopy,
  value,
}: Readonly<{
  copied: boolean;
  label: string;
  onCopy: () => void;
  value: string;
}>) {
  return (
    <div className="code-panel">
      <div className="code-panel-head">
        <span>{label}</span>
        <button className="copy-button" onClick={onCopy} type="button">
          {copied ? <CheckIcon /> : <CopyIcon />}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>
        <code>{value}</code>
      </pre>
    </div>
  );
}

export function WorkflowLab() {
  const [format, setFormat] = useState<SchemaFormat>("protobuf");
  const [selectedStep, setSelectedStep] = useState(0);
  const [completedThrough, setCompletedThrough] = useState(-1);
  const [copiedTarget, setCopiedTarget] = useState<"command" | "source" | null>(null);

  const example = formats[format];
  const step = steps[selectedStep];
  const progress = ((completedThrough + 1) / steps.length) * 100;
  const isCompleted = selectedStep <= completedThrough;
  const isAvailable = selectedStep <= completedThrough + 1;

  const coordinateManifest = useMemo(
    () => `{
  "schemahub_revision":
    "projects/codelab/repos/registry/revisions/ab0c5d…ab7ac",
  "schema_path": "${example.schemaPath}",
  "artifact_kind": "descriptors",
  "artifact_digest": "sha256:a5bd…0f52f"
}`,
    [example.schemaPath],
  );

  const selectFormat = (nextFormat: SchemaFormat) => {
    setFormat(nextFormat);
    setSelectedStep(0);
    setCompletedThrough(-1);
    setCopiedTarget(null);
  };

  const runSelectedStep = () => {
    if (!isAvailable) {
      return;
    }
    if (isCompleted) {
      if (selectedStep < steps.length - 1) {
        setSelectedStep(selectedStep + 1);
      }
      return;
    }
    setCompletedThrough(selectedStep);
  };

  const resetLab = () => {
    setSelectedStep(0);
    setCompletedThrough(-1);
    setCopiedTarget(null);
  };

  const copyValue = async (target: "command" | "source", value: string) => {
    await navigator.clipboard.writeText(value);
    setCopiedTarget(target);
    window.setTimeout(() => setCopiedTarget(null), 1600);
  };

  const actionLabel = (() => {
    if (!isAvailable) {
      return "Complete earlier steps";
    }
    if (isCompleted && selectedStep === steps.length - 1) {
      return "Workflow complete";
    }
    if (isCompleted) {
      return `Continue to step ${selectedStep + 2}`;
    }
    if (step.actor === "human") {
      return "Approve as human";
    }
    if (step.actor === "consumer") {
      return "Fetch immutable artifact";
    }
    return "Run this step";
  })();

  return (
    <main>
      <header className="site-header">
        <a aria-label="SchemaHub home" className="brand" href="#top">
          <GridMark />
          <span>SchemaHub</span>
          <em>Workflow Lab</em>
        </a>
        <nav aria-label="Primary navigation">
          <a href="#scenarios">Scenarios</a>
          <a href="#workflow">Workflow</a>
          <a href="#contract">Data contract</a>
          <a href="#boundaries">Boundaries</a>
        </nav>
        <a
          className="header-link"
          href="https://github.com/Shuozeli/schemahub"
          rel="noreferrer"
          target="_blank"
        >
          View source <ArrowIcon />
        </a>
      </header>

      <section className="hero" id="top">
        <div className="hero-copy">
          <p className="kicker">
            <span>Interactive codelab</span>
            Human + agent schema operations
          </p>
          <h1>
            Schema changes are
            <span> conversations </span>
            before they are commits.
          </h1>
          <p className="hero-lede">
            Walk an agent-authored proposal through compiler validation, human review,
            idempotent Apply, and immutable artifact serving—without confusing schema
            storage with application-data storage.
          </p>
          <div className="hero-actions">
            <a className="button button-primary" href="#workflow">
              Start the workflow <ArrowIcon />
            </a>
            <a
              className="button button-secondary"
              href="https://github.com/Shuozeli/schemahub/tree/agent/schemahub-usage-codelab/codelabs/real-world"
              rel="noreferrer"
              target="_blank"
            >
              Run the codelab suite
            </a>
          </div>
        </div>

        <div className="hero-contract" aria-label="Example immutable schema contract">
          <div className="contract-topline">
            <span className="live-dot" />
            Immutable serving plane
            <span>schemahub.v1</span>
          </div>
          <div className="contract-grid">
            <div>
              <span>mutable input</span>
              <strong>main</strong>
            </div>
            <div>
              <span>resolved once</span>
              <strong>ab0c5d…ab7ac</strong>
            </div>
          </div>
          <div className="digest-block">
            <span>artifact digest</span>
            <code>sha256:a5bd098c494c…6490f52f</code>
          </div>
          <div className="contract-route">
            <span>Agent intent</span>
            <i />
            <span>Human gate</span>
            <i />
            <span>Verified bytes</span>
          </div>
        </div>
      </section>

      <section className="signal-strip" aria-label="Product guarantees">
        <div>
          <strong>2</strong>
          <span>distinct identities</span>
        </div>
        <div>
          <strong>1</strong>
          <span>durable ChangeRecord</span>
        </div>
        <div>
          <strong>0</strong>
          <span>review bypasses</span>
        </div>
        <div>
          <strong>∞</strong>
          <span>safe artifact reads</span>
        </div>
      </section>

      <section className="scenarios-section" id="scenarios">
        <div className="section-heading">
          <div>
            <p className="section-index">01 — Validation portfolio</p>
            <h2>Use reality to find the bugs.</h2>
          </div>
          <p>
            The interactive lesson is paired with four executable domain labs.
            Each starts a release server, compiles served bindings, preserves
            evidence, and exercises a recognizable producer or operator workflow.
          </p>
        </div>

        <div className="portfolio-status">
          <span><i /> 5 scenarios passing</span>
          <span>4 executable domain codelabs</span>
          <span>Real CLI · real compiler · reproducible evidence</span>
        </div>

        <div className="scenario-grid" data-testid="scenario-portfolio">
          <article className="scenario-card scenario-live">
            <div className="scenario-card-top">
              <span>01</span>
              <strong>Passing</strong>
            </div>
            <h3>Human + agent approval</h3>
            <p>
              Delegated intent, compiler validation, a human policy gate,
              idempotent Apply, and immutable artifact fetch.
            </p>
            <a href="#workflow">Open interactive scenario <ArrowIcon /></a>
          </article>
          <article className="scenario-card">
            <div className="scenario-card-top">
              <span>02</span>
              <strong>Passing</strong>
            </div>
            <h3>Commerce contract rollout</h3>
            <p>
              Protobuf additive evolution, breaking-change rejection, generated
              bindings, and digest-pinned order data.
            </p>
            <a
              href="https://github.com/Shuozeli/schemahub/blob/agent/schemahub-usage-codelab/docs/codelab-commerce-protobuf.md"
              rel="noreferrer"
              target="_blank"
            >
              Open runnable codelab <ArrowIcon />
            </a>
          </article>
          <article className="scenario-card">
            <div className="scenario-card-top">
              <span>03</span>
              <strong>Passing</strong>
            </div>
            <h3>Mobile telemetry evolution</h3>
            <p>
              FlatBuffers defaults, field deprecation, old/new readers, and
              byte-stable generated artifacts.
            </p>
            <a
              href="https://github.com/Shuozeli/schemahub/blob/agent/schemahub-usage-codelab/docs/codelab-mobile-telemetry-flatbuffers.md"
              rel="noreferrer"
              target="_blank"
            >
              Open runnable codelab <ArrowIcon />
            </a>
          </article>
          <article className="scenario-card">
            <div className="scenario-card-top">
              <span>04</span>
              <strong>Passing</strong>
            </div>
            <h3>Concurrent editors</h3>
            <p>
              Human and agent races, stale ETags, conflicts, retry identity, and
              restart recovery.
            </p>
            <a
              href="https://github.com/Shuozeli/schemahub/blob/agent/schemahub-usage-codelab/docs/codelab-concurrent-human-agent.md"
              rel="noreferrer"
              target="_blank"
            >
              Open runnable codelab <ArrowIcon />
            </a>
          </article>
          <article className="scenario-card">
            <div className="scenario-card-top">
              <span>05</span>
              <strong>Passing</strong>
            </div>
            <h3>Data-pipeline handoff</h3>
            <p>
              Producer/consumer coordination, immutable serving, revision
              sidecars, digest verification, and rollback.
            </p>
            <a
              href="https://github.com/Shuozeli/schemahub/blob/agent/schemahub-usage-codelab/docs/codelab-data-pipeline-handoff.md"
              rel="noreferrer"
              target="_blank"
            >
              Open runnable codelab <ArrowIcon />
            </a>
          </article>
        </div>
      </section>

      <section className="workflow-section" id="workflow">
        <div className="section-heading">
          <div>
            <p className="section-index">02 — Guided workflow</p>
            <h2>Run the collaboration loop.</h2>
          </div>
          <p>
            This is a deterministic simulation of the real CLI contract. Pick a format,
            inspect each actor's command, and move the record through its guarded states.
          </p>
        </div>

        <div className="format-switch" role="group" aria-label="Schema format">
          {(Object.keys(formats) as SchemaFormat[]).map((formatId) => (
            <button
              aria-pressed={format === formatId}
              className={format === formatId ? "active" : ""}
              key={formatId}
              onClick={() => selectFormat(formatId)}
              type="button"
            >
              <span>{formats[formatId].shortLabel}</span>
              {formats[formatId].label}
            </button>
          ))}
        </div>

        <div className="lab-shell">
          <aside className="step-rail" aria-label="Workflow steps">
            <div className="progress-head">
              <span>ChangeRecord lifecycle</span>
              <strong>{completedThrough + 1}/{steps.length}</strong>
            </div>
            <div className="progress-track">
              <span style={{ width: `${progress}%` }} />
            </div>
            <ol>
              {steps.map((workflowStep, index) => {
                const complete = index <= completedThrough;
                const available = index <= completedThrough + 1;
                const selected = index === selectedStep;
                return (
                  <li key={workflowStep.id}>
                    <button
                      aria-current={selected ? "step" : undefined}
                      className={`${selected ? "selected" : ""} ${
                        complete ? "complete" : ""
                      } ${available ? "available" : "locked"}`}
                      onClick={() => setSelectedStep(index)}
                      type="button"
                    >
                      <span className="step-number">
                        {complete ? <CheckIcon /> : String(index + 1).padStart(2, "0")}
                      </span>
                      <span className="step-name">
                        <small>{workflowStep.eyebrow}</small>
                        <strong>{workflowStep.title}</strong>
                      </span>
                      <span className={`mini-actor actor-${workflowStep.actor}`}>
                        {actorGlyphs[workflowStep.actor]}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ol>
            <button className="reset-button" onClick={resetLab} type="button">
              Reset simulation
            </button>
          </aside>

          <article className={`step-detail detail-${step.actor}`}>
            <div className="detail-header">
              <div>
                <ActorBadge actor={step.actor} />
                <p>{step.eyebrow}</p>
                <h3>{step.title}</h3>
              </div>
              <span className="status-chip">{statusLabels[step.status]}</span>
            </div>
            <p className="step-explanation">{step.detail}</p>

            {step.actor === "human" && (
              <div className="gate-note">
                <span>Policy gate</span>
                An Apply attempted before this approval returns
                <code> FAILED_PRECONDITION</code> without changing the ETag.
              </div>
            )}

            {!isAvailable && (
              <div className="locked-note">
                This step depends on the earlier lifecycle transitions. You can inspect it
                now, but complete the preceding steps before running it.
              </div>
            )}

            <div className="detail-code-grid">
              <CodePanel
                copied={copiedTarget === "command"}
                label={`${actorLabels[step.actor]} command`}
                onCopy={() => copyValue("command", step.command(example))}
                value={step.command(example)}
              />
              <div className="code-panel output-panel">
                <div className="code-panel-head">
                  <span>Expected resource state</span>
                  <span className={isCompleted ? "output-live" : "output-pending"}>
                    {isCompleted ? "recorded" : "preview"}
                  </span>
                </div>
                <pre>
                  <code>{step.output(example)}</code>
                </pre>
              </div>
            </div>

            <div className="detail-actions">
              <button
                className="button button-primary"
                disabled={!isAvailable || (isCompleted && selectedStep === steps.length - 1)}
                onClick={runSelectedStep}
                type="button"
              >
                {actionLabel} <ArrowIcon />
              </button>
              <span>
                ETags advance on writes. Stale clients fail closed with
                <code> ABORTED</code>.
              </span>
            </div>
          </article>

          <aside className="event-ledger" aria-label="Recorded audit events">
            <div className="ledger-heading">
              <span>Live audit ledger</span>
              <i className={completedThrough >= 0 ? "active" : ""} />
            </div>
            {completedThrough < 0 ? (
              <div className="ledger-empty">
                <span>∅</span>
                Run step 1 to record the first durable event.
              </div>
            ) : (
              <ol>
                {steps.slice(0, completedThrough + 1).map((event, index) => (
                  <li key={event.id}>
                    <span>{String(index + 1).padStart(2, "0")}</span>
                    <div>
                      <small>{event.actor}</small>
                      <strong>schemahub.change.{event.status}</strong>
                    </div>
                    <time>+{index * 7 + 2}ms</time>
                  </li>
                ))}
              </ol>
            )}
            {completedThrough === steps.length - 1 && (
              <div className="ledger-success">
                <CheckIcon />
                Revision and artifact digest are now safe to persist with data.
              </div>
            )}
          </aside>
        </div>
      </section>

      <section className="contract-section" id="contract">
        <div className="section-heading contract-heading">
          <div>
            <p className="section-index">03 — Data contract</p>
            <h2>Store coordinates, not assumptions.</h2>
          </div>
          <p>
            Your database owns business data. SchemaHub owns the versioned contract that
            explains those bytes. Persist both the revision and digest with the record,
            batch, object, or stream segment.
          </p>
        </div>

        <div className="contract-demo">
          <div className="source-card">
            <div className="card-label">
              <span>Proposed source</span>
              <strong>{example.shortLabel}</strong>
            </div>
            <CodePanel
              copied={copiedTarget === "source"}
              label={example.schemaPath}
              onCopy={() => copyValue("source", example.schemaSource)}
              value={example.schemaSource}
            />
          </div>

          <div className="contract-bridge" aria-hidden="true">
            <span>resolve</span>
            <i />
            <span>verify</span>
          </div>

          <div className="manifest-card">
            <div className="card-label">
              <span>Stored beside application data</span>
              <strong>schema pointer</strong>
            </div>
            <pre>
              <code>{coordinateManifest}</code>
            </pre>
            <div className="manifest-result">
              <div>
                <span>Descriptor</span>
                <strong>{example.descriptorLabel}</strong>
              </div>
              <div>
                <span>Generated symbol</span>
                <strong>{example.generatedSymbol}</strong>
              </div>
              <p>
                Restart or upgrade the server: the first-materialized artifact remains
                byte-identical for this request identity.
              </p>
            </div>
          </div>
        </div>
      </section>

      <section className="boundaries-section" id="boundaries">
        <div className="section-heading boundary-heading">
          <div>
            <p className="section-index">04 — Honest boundaries</p>
            <h2>A focused 1.0 contract.</h2>
          </div>
          <p>
            The useful part of a platform is knowing what it guarantees—and what remains
            an explicit coordination task for humans and agents.
          </p>
        </div>
        <div className="boundary-grid">
          <article className="guarantee-card">
            <span>Guaranteed</span>
            <h3>Inside the serving contract</h3>
            <ul>
              <li>Durable human and agent attribution</li>
              <li>Compiler-backed validation and compatibility</li>
              <li>Policy-gated, idempotent Apply</li>
              <li>Immutable artifact bytes and SHA-256 digests</li>
              <li>Protobuf and FlatBuffers generated code</li>
            </ul>
          </article>
          <article className="boundary-card">
            <span>Explicit boundary</span>
            <h3>Still coordinated by callers</h3>
            <ul>
              <li>No automatic cross-repository rewrite</li>
              <li>No global multi-repository transaction</li>
              <li>Repository-scoped search and bounded discovery</li>
              <li>No OpenAPI client/server code generation in 1.0</li>
              <li>GUI reviews mutations but does not author them directly</li>
            </ul>
          </article>
        </div>
      </section>

      <section className="closing-cta">
        <div>
          <p className="section-index">Ready to use the real service?</p>
          <h2>Run the same workflow against SchemaHub.</h2>
        </div>
        <div className="closing-actions">
          <a
            className="button button-primary"
            href="https://github.com/Shuozeli/schemahub/pull/4"
            rel="noreferrer"
            target="_blank"
          >
            Open the runnable codelab <ArrowIcon />
          </a>
          <a
            className="button button-secondary"
            href="https://github.com/Shuozeli/schemahub"
            rel="noreferrer"
            target="_blank"
          >
            Explore the repository
          </a>
        </div>
      </section>

      <footer>
        <a className="brand footer-brand" href="#top">
          <GridMark />
          <span>SchemaHub</span>
        </a>
        <p>Change control for humans and agents. Immutable schema serving for data.</p>
        <span>Interactive demo · no production data is sent</span>
      </footer>
    </main>
  );
}
