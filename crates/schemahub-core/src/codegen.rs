//! Codegen API (design.md §10): build the transitive import closure, then hand
//! it to the compiler for descriptor / source generation.

use bytes::Bytes;

use schemahub_jj::RefSpec;
use schemahub_types::{CodegenOptions, Language, SchemaPath};

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
        let (bytes, _) = self.generate_descriptors_resolved(schema, at, token)?;
        Ok(bytes)
    }

    /// Generate descriptors from one immutable, repository-owned snapshot and
    /// return the exact commit used for the payload.
    pub fn generate_descriptors_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        token: Option<&str>,
    ) -> CoreResult<(Bytes, String)> {
        let commit_id = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let closure = closure::build(self, schema, &commit_id, token)?;
        Ok((compiler.generate_descriptors(&closure)?, commit_id))
    }

    /// Render generated code for a schema closure in `lang` (PreviewCodegen).
    pub fn generate_code(&self, req: CodegenRequest, token: Option<&str>) -> CoreResult<String> {
        self.preview_codegen_at(
            &req.schema,
            &RefSpec::bookmark(&req.bookmark),
            req.lang,
            &req.options,
            token,
        )
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
        self.preview_codegen_at(
            schema,
            &RefSpec::bookmark(bookmark),
            lang,
            &CodegenOptions::default(),
            token,
        )
    }

    /// Render generated code at any ref (branch, tag, or commit).
    pub fn preview_codegen_at(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        lang: Language,
        options: &CodegenOptions,
        token: Option<&str>,
    ) -> CoreResult<String> {
        let (content, _) = self.preview_codegen_resolved(schema, at, lang, options, token)?;
        Ok(content)
    }

    /// Render code from one immutable, repository-owned snapshot and return
    /// the exact commit used for the payload.
    pub fn preview_codegen_resolved(
        &self,
        schema: &SchemaPath,
        at: &RefSpec,
        lang: Language,
        options: &CodegenOptions,
        token: Option<&str>,
    ) -> CoreResult<(String, String)> {
        let commit_id = self.resolve_read_commit(&schema.project, &schema.repo, at, token)?;
        let compiler = self.compiler_for(&schema.schema_name)?;
        let closure = closure::build(self, schema, &commit_id, token)?;
        Ok((compiler.generate_code(&closure, lang, options)?, commit_id))
    }
}
