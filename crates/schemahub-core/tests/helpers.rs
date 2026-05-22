/// Shared test helpers for schemahub-core integration tests.
use std::sync::Arc;

use schemahub_core::{Core, PluginRegistry, RepoConfig};
use schemahub_storage::RedbBackend;
use schemahub_types::{NoopAuthn, NoopAuthz};
use uuid::Uuid;

pub use schemahub_core::mutation::batch::BatchMutateRequest;
pub use schemahub_types::Mutation;

// ── Fresh in-process storage ──────────────────────────────────────────────────

pub fn ephemeral_storage() -> Arc<RedbBackend> {
    let path = format!("/tmp/schemahub-integration-{}.redb", Uuid::new_v4());
    Arc::new(RedbBackend::open(&path).unwrap())
}

// ── Plugin registry with all three format plugins ─────────────────────────────

pub fn all_plugins() -> PluginRegistry {
    let mut reg = PluginRegistry::new();
    reg.register(Arc::new(schemahub_plugin_protobuf::ProtobufPlugin));
    reg.register(Arc::new(schemahub_plugin_flatbuffers::FlatBuffersPlugin));
    reg.register(Arc::new(schemahub_plugin_openapi::OpenApiPlugin));
    reg
}

// ── Build a Core with all plugins ─────────────────────────────────────────────

pub fn make_core() -> Core {
    let storage = ephemeral_storage();
    Core::new(
        storage,
        all_plugins(),
        Arc::new(NoopAuthn),
        Arc::new(NoopAuthz),
    )
}

// ── Convenience: initialize a repo and return core ────────────────────────────

pub fn make_core_with_repo(project: &str, repo: &str) -> Core {
    let core = make_core();
    core.initialize_repo(project, repo, &RepoConfig::default()).unwrap();
    core
}

// ── Unique idempotency key ────────────────────────────────────────────────────

pub fn idem() -> String {
    Uuid::new_v4().to_string()
}
