//! Schema exploration / read API (design.md §9). Per-declaration storage makes
//! each read a direct object lookup; the core maps the stored blobs through the
//! compiler into summaries / details / type refs.

use schemahub_types::{Action, DeclDetail, DeclSummary, Import, SchemaPath, TypeRef};
use schemahub_vcs::RefSpec;

use crate::auth::authorize;
use crate::error::{CoreError, CoreResult};
use crate::request::{DeclLocation, SearchHit};
use crate::Core;

impl Core {
    /// List schema-file names in a repo at a ref (branch / tag / commit).
    pub fn list_schemas(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<String>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        Ok(self.vcs.list_schemas(project, repo, at)?)
    }

    /// List declaration summaries in a schema file at a ref (branch / tag /
    /// commit) (DeclBlob → DeclSummary via `compiler.summarize_decl`).
    pub fn list_declarations(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclSummary>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let names =
            self.vcs
                .list_declarations(&schema.project, &schema.repo, &schema.schema_name, at)?;
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let blob = self.vcs.get_declaration(
                &schema.project,
                &schema.repo,
                &schema.schema_name,
                &name,
                at,
            )?;
            out.push(compiler.summarize_decl(&blob)?);
        }
        Ok(out)
    }

    /// Fetch one declaration's detail at a ref (→ `compiler.decl_detail`).
    pub fn get_declaration(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<DeclDetail> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let blob = self.vcs.get_declaration(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            name,
            at,
        )?;
        Ok(compiler.decl_detail(&blob)?)
    }

    /// Resolve the type names a declaration references (FollowType, design.md §9).
    /// Combines the declaration's `type_refs` with the file's imports so a caller
    /// can locate where each referenced type is defined.
    pub fn follow_type(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        name: &str,
        token: Option<&str>,
    ) -> CoreResult<(Vec<TypeRef>, Vec<Import>)> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let blob = self.vcs.get_declaration(
            &schema.project,
            &schema.repo,
            &schema.schema_name,
            name,
            at,
        )?;
        let refs = compiler.type_refs(&blob)?;
        let meta = self
            .vcs
            .load_schema(&schema.project, &schema.repo, &schema.schema_name, at)?
            .meta;
        let imports = compiler.imports(&meta)?;
        Ok((refs, imports))
    }

    /// List the imports declared in a schema file (design.md §9 ListDependencies).
    pub fn list_dependencies(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<Vec<Import>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let meta = self
            .vcs
            .load_schema(&schema.project, &schema.repo, &schema.schema_name, at)?
            .meta;
        Ok(compiler.imports(&meta)?)
    }

    /// Reconstruct canonical source for a schema file at a ref (GetSchemaSource).
    pub fn get_schema_source(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
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
        let objs =
            self.vcs
                .load_schema(&schema.project, &schema.repo, &schema.schema_name, at)?;
        Ok(compiler.print(&objs)?)
    }

    /// Search for declarations by name across all schemas in a repo (design.md §9).
    /// `query` matches as a case-insensitive substring of the declaration name.
    pub fn search(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        query: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<SearchHit>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        for schema_name in self.vcs.list_schemas(project, repo, at)? {
            let names = self.vcs.list_declarations(project, repo, &schema_name, at)?;
            for decl_name in names {
                if decl_name.to_lowercase().contains(&needle) {
                    hits.push(SearchHit {
                        schema_name: schema_name.clone(),
                        decl_name,
                    });
                }
            }
        }
        Ok(hits)
    }

    /// Search returning full summaries (DeclLocation) for richer client display.
    pub fn search_detailed(
        &self,
        project: &str,
        repo: &str,
        at: &RefSpec,
        query: &str,
        token: Option<&str>,
    ) -> CoreResult<Vec<DeclLocation>> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            project,
            repo,
        )?;
        let needle = query.to_lowercase();
        let mut hits = Vec::new();
        for schema_name in self.vcs.list_schemas(project, repo, at)? {
            let compiler = match self.compiler_for(&schema_name) {
                Ok(c) => c,
                Err(_) => continue, // unknown format file in the repo — skip it
            };
            for decl_name in self.vcs.list_declarations(project, repo, &schema_name, at)? {
                if !decl_name.to_lowercase().contains(&needle) {
                    continue;
                }
                let blob = self
                    .vcs
                    .get_declaration(project, repo, &schema_name, &decl_name, at)?;
                hits.push(DeclLocation {
                    schema_name: schema_name.clone(),
                    summary: compiler.summarize_decl(&blob)?,
                });
            }
        }
        Ok(hits)
    }

    /// Resolve the compiler for a schema file by its extension.
    pub(crate) fn compiler_for(
        &self,
        schema_name: &str,
    ) -> CoreResult<std::sync::Arc<dyn schemahub_types::Compiler>> {
        let format_id = crate::detect_format_from_name(schema_name)
            .ok_or_else(|| CoreError::UndetectableFormat(schema_name.to_string()))?;
        self.registry
            .get(format_id)
            .cloned()
            .ok_or_else(|| CoreError::UnknownFormat(format_id.to_string()))
    }
}
