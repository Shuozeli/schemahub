//! Codegen API (design.md §10): build the transitive import closure, then hand
//! it to the compiler for descriptor / source generation.

use bytes::Bytes;

use schemahub_types::{Action, Language, SchemaPath};
use schemahub_vcs::RefSpec;

use crate::auth::authorize;
use crate::error::CoreResult;
use crate::mutation::closure;
use crate::request::CodegenRequest;
use crate::Core;

impl Core {
    /// Generate the native descriptor artifact for a schema and its closure
    /// (protobuf → FileDescriptorSet bytes, etc.) — GetDescriptors.
    pub fn generate_descriptors(
        &self,
        schema: &SchemaPath,
        bookmark: &str,
        token: Option<&str>,
    ) -> CoreResult<Bytes> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &schema.project,
            &schema.repo,
        )?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let closure = closure::build(
            &self.vcs,
            compiler.as_ref(),
            schema,
            &RefSpec::bookmark(bookmark),
        )?;
        Ok(compiler.generate_descriptors(&closure)?)
    }

    /// Render generated code for a schema closure in `lang` (PreviewCodegen).
    pub fn generate_code(&self, req: CodegenRequest, token: Option<&str>) -> CoreResult<String> {
        authorize(
            self.authn.as_ref(),
            self.authz.as_ref(),
            token,
            Action::Read,
            &req.schema.project,
            &req.schema.repo,
        )?;
        let compiler = self.compiler_for(&req.schema.schema_name)?;
        let closure = closure::build(
            &self.vcs,
            compiler.as_ref(),
            &req.schema,
            &RefSpec::bookmark(&req.bookmark),
        )?;
        Ok(compiler.generate_code(&closure, req.lang)?)
    }

    /// Convenience: generate code without a request wrapper (used by the CLI
    /// path and kept stable for the server contract).
    pub fn preview_codegen(
        &self,
        schema: &SchemaPath,
        bookmark: &str,
        lang: Language,
        token: Option<&str>,
    ) -> CoreResult<String> {
        self.generate_code(
            CodegenRequest {
                schema: schema.clone(),
                bookmark: bookmark.to_string(),
                lang,
            },
            token,
        )
    }
}
