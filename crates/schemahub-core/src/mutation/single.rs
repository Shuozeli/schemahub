//! Single-mutation flow (design.md §5.1).

use schemahub_jj::SchemaWrite;
use schemahub_types::Action;

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::idempotency::{force_audit_attributes, FingerprintBuilder};
use crate::mutation::{compat, immutable_bookmark_base, load_base};
use crate::request::{MutationRequest, MutationResponse};
use crate::Core;

impl Core {
    /// Single-mutation flow: authn/authz → load → compiler
    /// `apply_mutation` → compatibility gate (protected bookmarks) → one commit.
    pub fn apply_mutation(&self, req: MutationRequest) -> CoreResult<MutationResponse> {
        let path = &req.mutation.schema_path;
        let project = path.project.clone();
        let repo = path.repo.clone();
        let schema_name = path.schema_name.clone();

        // AuthN → AuthZ (Force if --force, else Write). Receipts are checked
        // only after authorization so they cannot become an information leak.
        let action = if req.force {
            Action::Force
        } else {
            Action::Write
        };
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            req.token.as_deref(),
            action,
            &project,
            &repo,
        )?;
        let mut fingerprint = FingerprintBuilder::new("apply-mutation");
        for field in [
            project.as_bytes(),
            repo.as_bytes(),
            req.bookmark.as_bytes(),
            schema_name.as_bytes(),
            req.mutation.format_id.as_bytes(),
            req.mutation.operation.as_ref(),
            req.author.as_bytes(),
            req.message.as_bytes(),
            req.base_revision.as_deref().unwrap_or_default().as_bytes(),
        ] {
            fingerprint.update(field);
        }
        fingerprint.update(&[u8::from(req.force)]);
        let fingerprint = fingerprint.finish();
        let scope = format!("apply-mutation/{project}/{repo}");
        if let Some(response) = self.replay_idempotent_write(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &project,
            &repo,
            &req.bookmark,
        )? {
            return Ok(response);
        }
        self.validate_base_revision(&project, &repo, req.base_revision.as_deref())?;

        // Select the compiler by format_id (core stays format-agnostic).
        let compiler = self
            .registry
            .get(&req.mutation.format_id)
            .ok_or_else(|| CoreError::UnknownFormat(req.mutation.format_id.clone()))?
            .clone();

        // Load the base schema (tolerating first-write / fresh bookmark).
        let base_ref = immutable_bookmark_base(&self.jj, &project, &repo, &req.bookmark)?;
        let base = load_base(&self.jj, &project, &repo, &schema_name, &base_ref)?;

        // Apply the typed op via the compiler.
        let effect = compiler.apply_mutation(&base, &req.mutation)?;

        // Compatibility gate on protected bookmarks (unless --force).
        self.ensure_direct_write_allowed(&project, &repo)?;
        let config = self.effective_repo_config(&project, &repo)?;
        if !req.force
            && schemahub_jj::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks)
        {
            compat::gate(compiler.as_ref(), &config.compat_rules(), &base, &effect)?;
        }

        // Claim the durable receipt immediately before the side effect, then
        // stamp its attempt id on the publishing JJ operation.
        self.commit_idempotent_schema_changes_with_attributes(
            &scope,
            req.idempotency_key.as_deref(),
            &fingerprint,
            &project,
            &repo,
            &req.bookmark,
            &base_ref,
            vec![SchemaWrite::Patch {
                schema_path: schema_name,
                effect,
            }],
            &req.author,
            &req.message,
            force_audit_attributes(req.force),
        )
    }
}
