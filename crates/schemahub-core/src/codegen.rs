//! Codegen API (design.md §10): build the transitive import closure, then hand
//! it to the compiler for descriptor / source generation.

use bytes::Bytes;

use schemahub_jj::RefSpec;
use schemahub_types::{Action, Language, SchemaPath};

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
        self.generate_descriptors_at(schema, &RefSpec::bookmark(bookmark), token)
    }

    /// Generate descriptors at any ref (branch, tag, or commit). This is the
    /// cloud-build path: descriptor caches need a stable resolved commit key.
    pub fn generate_descriptors_at(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
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
        let closure = closure::build(&self.jj, compiler.as_ref(), schema, at)?;
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
            &self.jj,
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
        self.preview_codegen_at(schema, &RefSpec::bookmark(bookmark), lang, token)
    }

    /// Render generated code at any ref (branch, tag, or commit).
    pub fn preview_codegen_at(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        lang: Language,
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
        let closure = closure::build(&self.jj, compiler.as_ref(), schema, at)?;
        Ok(compiler.generate_code(&closure, lang)?)
    }
}
