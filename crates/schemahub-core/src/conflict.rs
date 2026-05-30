//! First-class conflict orchestration (design.md §6): render a conflicted
//! declaration for display, and validate + apply a proposed resolution.

use schemahub_types::{Action, DeclBlob, SchemaPath};
use schemahub_vcs::RefSpec;

use crate::auth::authorize;
use crate::error::CoreResult;
use crate::request::MutationResponse;
use crate::Core;

impl Core {
    /// Render a conflicted declaration's competing sides for human/agent display
    /// (`vcs.read_conflict` → `compiler.render_conflict`).
    pub fn render_conflict(
        &self,
        schema: &SchemaPath,
        bookmark: &str,
        decl: &str,
        token: Option<&str>,
    ) -> CoreResult<String> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let sides = self.vcs.read_conflict(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            decl,
            &RefSpec::bookmark(bookmark),
        )?;
        Ok(compiler.render_conflict(&sides)?)
    }

    /// Validate a proposed resolution and commit it, replacing the conflict
    /// (`compiler.validate_resolution` → `vcs.resolve_conflict`).
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_conflict(
        &self,
        schema: &SchemaPath,
        bookmark: &str,
        decl: &str,
        resolved: DeclBlob,
        author: &str,
        message: &str,
        token: Option<&str>,
    ) -> CoreResult<MutationResponse> {
        // Resolving a conflict is a write.
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Write,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        compiler.validate_resolution(&resolved)?;
        let write = self.vcs.resolve_conflict(
            &schema.project,
            &schema.repo,
            bookmark,
            &schema.schema_name,
            decl,
            resolved,
            author,
            message,
        )?;
        Ok(MutationResponse {
            commit_id: write.commit_id,
            change_id: write.change_id,
            conflicted_decls: write.conflicted_decls,
        })
    }
}
