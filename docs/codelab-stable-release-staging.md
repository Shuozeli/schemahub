<!-- agent-updated: 2026-07-30T04:16:42Z -->
# Attest a Stable Release in Production-Like Staging

This codelab turns SchemaHub's production acceptance checklist into the
machine-checked input required by a stable tag. It is for a release operator
promoting a prerelease candidate to `vMAJOR.MINOR.PATCH`.

A prerelease tag can publish without this attestation so its image can be
tested. A stable tag builds and pushes its image under a unique candidate tag,
then pauses at the protected `schemahub-production-staging` environment. The
semantic-version container tag and GitHub release are not published unless the
supplied attestation matches the exact stable source, container digest, and
retained GA-readiness archive.

The attestation is evidence metadata, not a credential container. Never place
access tokens, passwords, private keys, cookies, or bearer headers in it.

## 1. Protect the GitHub environment

Create the `schemahub-production-staging` environment in repository settings
before starting a stable release. Configure:

- at least one required reviewer who did not perform the staging run;
- deployment-branch/tag rules restricted to release tags;
- no bypass for the release operator; and
- the environment variable
  `SCHEMAHUB_STAGING_ATTESTATION_B64`, populated only after the exact stable
  image has passed this codelab.

Do not define the attestation variable at repository or organization scope.
The workflow uses the named environment so approval and evidence stay coupled.
An absent or invalid value fails the stable release closed.

After creating the environment and its independent-review rule, create the
single custom deployment policy as a tag rule:

```bash
gh api \
  --method POST \
  --header 'X-GitHub-Api-Version: 2026-03-10' \
  repos/Shuozeli/schemahub/environments/schemahub-production-staging/deployment-branch-policies \
  -f name='v*.*.*' \
  -f type='tag'
```

Audit both environment resources before starting a stable release:

```bash
gh api \
  --header 'X-GitHub-Api-Version: 2026-03-10' \
  repos/Shuozeli/schemahub/environments/schemahub-production-staging \
  > schemahub-staging-environment.json
gh api \
  --header 'X-GitHub-Api-Version: 2026-03-10' \
  'repos/Shuozeli/schemahub/environments/schemahub-production-staging/deployment-branch-policies?per_page=100' \
  > schemahub-staging-deployment-policies.json
scripts/validate-staging-environment.sh \
  schemahub-staging-environment.json \
  schemahub-staging-deployment-policies.json
```

The validator requires exactly one `v*.*.*` policy and rejects a missing,
broader, branch-typed, or additional policy. The release workflow repeats this
audit through GitHub's Actions-read API before it trusts the environment.

## 2. Resolve the exact stable coordinates

Push the authorized stable tag and let the release workflow finish its full CI
and container jobs. The staging job will wait for environment approval.

Set the workflow and release coordinates from that run:

```bash
export RELEASE_RUN_ID=123456789
export RELEASE_VERSION=1.0.0
export SOURCE_REVISION="$(git rev-list -n 1 "v$RELEASE_VERSION")"
export CONTAINER_IMAGE=ghcr.io/shuozeli/schemahub
export RELEASE_RUN_ATTEMPT="$(
  gh api \
    "repos/Shuozeli/schemahub/actions/runs/$RELEASE_RUN_ID" \
    --jq .run_attempt
)"
export CONTAINER_CANDIDATE="$(
  printf '%s:candidate-%s-%s' \
    "$CONTAINER_IMAGE" \
    "$RELEASE_RUN_ID" \
    "$RELEASE_RUN_ATTEMPT"
)"
export CONTAINER_DIGEST="$(
  docker buildx imagetools inspect \
    "$CONTAINER_CANDIDATE" \
    | awk '$1 == "Digest:" { print $2; exit }'
)"

test "$(printf '%s' "$SOURCE_REVISION" | wc -c)" -eq 40
test "$(printf '%s' "${CONTAINER_DIGEST#sha256:}" | wc -c)" -eq 64
```

Download the exact scenario evidence produced inside the same release run and
record its digest:

```bash
mkdir -p staging-input
gh run download "$RELEASE_RUN_ID" \
  --name schemahub-ga-readiness \
  --dir staging-input
export GA_READINESS_DIGEST="sha256:$(
  sha256sum staging-input/schemahub-ga-readiness.tar.gz | awk '{print $1}'
)"
```

Deploy by digest, never by a mutable tag:

```bash
docker pull "$CONTAINER_IMAGE@$CONTAINER_DIGEST"
docker image inspect "$CONTAINER_IMAGE@$CONTAINER_DIGEST" \
  --format '{{index .RepoDigests 0}}'
```

Use PostgreSQL and the intended external JWT/JWKS provider. Bind host ports to
the Tailscale address and address the deployment through its fully qualified
MagicDNS hostname as shown in `codelab-deploy.md`.

## 3. Run production acceptance

Retain secret-free transcripts and results for these checks:

1. A delegated agent authors an executable change, a human reviews it, and the
   agent applies it.
2. The exact image serves the bundled GUI from its HTTPS root, a nested
   `/projects/...` route loads directly, its hashed entry asset has immutable
   caching, no code-viewer or other application asset is fetched from a remote
   CDN, and an unknown `/api/*` path remains a non-HTML `404`.
3. Source, descriptor, and generated artifact bytes remain identical after a
   process restart.
4. An artifact first served by the prior candidate remains byte-identical.
5. A deliberately corrupt durable artifact record fails closed instead of
   being silently rerendered.
6. `ListDependents` reports live and pinned visible consumers without
   disclosing a hidden repository.
7. A PostgreSQL backup restores into a fresh database and returns identical
   immutable artifact bytes.
8. The intended identity provider accepts both current and next signing keys,
   rejects the removed key, changes `/readyz` to `503` and rejects credentials
   after the JWKS stale bound, then recovers after valid JWKS returns.

Development static credentials do not satisfy this codelab. The complete
deployment, identity, artifact, and backup commands are in
`codelab-deploy.md`, `authentication.md`, and `codelab-operations.md`.

Package the sanitized evidence, publish it at an access-controlled HTTPS URL
that the release reviewers can inspect, and calculate its SHA-256 digest:

```bash
export STAGING_EVIDENCE_URL=https://evidence.example.com/schemahub/staging-1.0.0.tar.gz
export STAGING_EVIDENCE_DIGEST=sha256:replace-with-evidence-bundle-digest
export STAGING_RUN_ID=staging-1.0.0-1
export STAGING_OPERATOR=release-owner
export STAGING_URL=https://schemahub-staging.example.com
export IDENTITY_ISSUER=https://identity.example.com
export DEPLOYED_AT=2026-07-24T04:30:00Z
export ATTESTED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
```

Both timestamps must be UTC RFC 3339 values ending in `Z`. The deployment must
precede the attestation, and both must be no more than seven days old when the
workflow validates them.

## 4. Create and validate the attestation

Generate the exact versioned shape:

```bash
jq -n \
  --arg attested_at "$ATTESTED_AT" \
  --arg version "$RELEASE_VERSION" \
  --arg source_revision "$SOURCE_REVISION" \
  --arg container_image "$CONTAINER_IMAGE" \
  --arg container_digest "$CONTAINER_DIGEST" \
  --arg ga_readiness_digest "$GA_READINESS_DIGEST" \
  --arg deployment_url "$STAGING_URL" \
  --arg deployed_at "$DEPLOYED_AT" \
  --arg issuer "$IDENTITY_ISSUER" \
  --arg evidence_url "$STAGING_EVIDENCE_URL" \
  --arg evidence_digest "$STAGING_EVIDENCE_DIGEST" \
  --arg run_id "$STAGING_RUN_ID" \
  --arg operator "$STAGING_OPERATOR" '{
    schema_version: "schemahub.staging-attestation.v1",
    attested_at: $attested_at,
    release: {
      version: $version,
      source_revision: $source_revision,
      container_image: $container_image,
      container_digest: $container_digest,
      ga_readiness_digest: $ga_readiness_digest
    },
    deployment: {
      url: $deployment_url,
      backend: "postgres",
      exact_digest: true,
      deployed_at: $deployed_at
    },
    identity: {
      issuer: $issuer,
      development_credentials_used: false,
      current_key_accepted: true,
      next_key_accepted: true,
      removed_key_rejected: true,
      stale_keys_readyz_503: true,
      stale_keys_credentials_rejected: true,
      recovered_after_valid_jwks: true
    },
    acceptance: {
      human_agent_workflow: true,
      bundled_gui_same_origin: true,
      restart_bytes_identical: true,
      prior_candidate_bytes_identical: true,
      corrupt_artifact_failed_closed: true,
      list_dependents_live_pinned_hidden: true,
      backup_restore_drill: true
    },
    evidence: {
      url: $evidence_url,
      digest: $evidence_digest,
      run_id: $run_id,
      operator: $operator
    }
  }' > schemahub-staging-attestation.json
```

Validate the same contract locally:

```bash
scripts/validate-staging-attestation.sh \
  schemahub-staging-attestation.json \
  "$RELEASE_VERSION" \
  "$SOURCE_REVISION" \
  "$CONTAINER_IMAGE" \
  "$CONTAINER_DIGEST" \
  "$GA_READINESS_DIGEST"
```

The validator rejects missing or extra fields, coordinate drift, incomplete
product or bundled-GUI checks, development credentials, credential-shaped
content, non-HTTPS evidence, invalid digests, future timestamps, and stale
evidence.

## 5. Submit and approve

Encode the validated JSON without line breaks and set it only on the protected
environment:

```bash
base64 < schemahub-staging-attestation.json | tr -d '\n' \
  | gh variable set SCHEMAHUB_STAGING_ATTESTATION_B64 \
      --env schemahub-production-staging \
      --repo Shuozeli/schemahub
```

The independent reviewer should compare the environment value's decoded
coordinates with the release run and inspect the evidence URL before approving
the waiting deployment.

After approval, the workflow validates the attestation again, normalizes it,
and uploads `schemahub-staging-attestation.json`. It assembles and retains the
release notes, both SBOMs, archives, evidence, and `SHA256SUMS` before creating
`$CONTAINER_IMAGE:$RELEASE_VERSION` from the attested digest. Promotion refuses
to overwrite a different existing version tag and verifies the final registry
digest. The assembly must contain only safe, checksummed regular files and is
verified before upload and again after download. Stable GitHub publication
consumes only that reverified assembly and proceeds only if promotion succeeds.
The attestation is included alongside the GA-readiness archive.

## 6. Verify publication

After the workflow succeeds:

```bash
test "$(
  docker buildx imagetools inspect \
    "$CONTAINER_IMAGE:$RELEASE_VERSION" \
    | awk '$1 == "Digest:" { print $2; exit }'
)" = "$CONTAINER_DIGEST"
gh release download "v$RELEASE_VERSION" \
  --pattern schemahub-staging-attestation.json \
  --pattern SHA256SUMS \
  --dir published-release
(
  cd published-release
  sha256sum --check --ignore-missing SHA256SUMS
)
```

Confirm the published JSON still names the exact source, image digest,
GA-readiness digest, evidence bundle, and operator that the reviewer accepted.
