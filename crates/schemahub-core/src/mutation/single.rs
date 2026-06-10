//! Single-mutation flow (design.md §5.1).

use schemahub_jj::RefSpec;
use schemahub_types::Action;

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::mutation::{compat, load_base};
use crate::request::{MutationRequest, MutationResponse};
use crate::Core;

impl Core {
    /// Single-mutation flow: idempotency → authn/authz → load → compiler
    /// `apply_mutation` → compatibility gate (protected bookmarks) → one commit.
    pub fn apply_mutation(&self, req: MutationRequest) -> CoreResult<MutationResponse> {
        // 1. Idempotency dedupe at the edge.
        if let Some(key) = &req.idempotency_key {
            if let Some(stored) = self.idempotency.get(key) {
                return Ok(stored);
            }
        }

        let path = &req.mutation.schema_path;
        let project = path.project.clone();
        let repo = path.repo.clone();
        let schema_name = path.schema_name.clone();

        // 2. AuthN → 3. AuthZ (Force if --force, else Write).
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

        // Select the compiler by format_id (core stays format-agnostic).
        let compiler = self
            .registry
            .get(&req.mutation.format_id)
            .ok_or_else(|| CoreError::UnknownFormat(req.mutation.format_id.clone()))?
            .clone();

        // 4. Load the base schema (tolerating first-write / fresh bookmark).
        let base_ref = RefSpec::bookmark(req.bookmark.clone());
        let base = load_base(&self.jj, &project, &repo, &schema_name, &base_ref)?;

        // 5. Apply the typed op via the compiler.
        let effect = compiler.apply_mutation(&base, &req.mutation)?;

        // 6. Compatibility gate on protected bookmarks (unless --force).
        let config = self.repo_configs.get(&project, &repo);
        if !req.force
            && schemahub_jj::bookmark::is_protected(&req.bookmark, &config.protected_bookmarks)
        {
            compat::gate(compiler.as_ref(), &config.compat_rules(), &base, &effect)?;
        }

        // 7. Commit the effect under one operation; concurrency yields conflicts,
        //    never a CAS rejection (design.md §5.1).
        let write = self.jj.commit_write(
            &project,
            &repo,
            &req.bookmark,
            &schema_name,
            &base_ref,
            effect,
            &req.author,
            &req.message,
        )?;

        let response = MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        };

        // 8. Store the idempotency result.
        if let Some(key) = &req.idempotency_key {
            self.idempotency.put(key, response.clone());
        }

        Ok(response)
    }
}
