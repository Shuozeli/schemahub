//! Postgres-backed [`ObjectDb`] — the multi-instance/server alternative to
//! the embedded redb default (design.md §4.5).
//!
//! ## Schema
//!
//! Four tables, created idempotently on `connect`:
//!
//! - `objects(kind INT NOT NULL, id BYTEA PRIMARY KEY, bytes BYTEA NOT NULL)` —
//!   content-addressed; the kind is informational only since `id` (the content
//!   hash) is globally unique. We still filter by kind in
//!   [`ObjectDb::list_objects`] / [`ObjectDb::delete_object`] /
//!   [`ObjectDb::get_object`] so callers see the same per-kind view they get
//!   from the in-memory and redb backends.
//! - `ops(repo TEXT, op_id BYTEA, op_bytes BYTEA, inserted_at BIGSERIAL,
//!   PRIMARY KEY(repo, op_id))` — per-repo op-log. `inserted_at` is a per-row
//!   monotonic sequence so [`ObjectDb::list_ops`] returns oldest→newest
//!   deterministically, matching the in-memory backend's semantics whenever a
//!   single writer streams ops in.
//! - `refs(repo TEXT, name TEXT, target BYTEA, PRIMARY KEY(repo, name))` —
//!   per-repo named refs.
//! - `resource_records(collection TEXT, record_key TEXT, record_bytes BYTEA,
//!   PRIMARY KEY(collection, record_key))` — mutable control-plane resources
//!   with atomic compare-and-swap updates.
//!
//! ## Sync ↔ async bridge
//!
//! The [`ObjectDb`] trait is synchronous while SQLx is async-only. Each
//! `PgObjectDb` therefore owns one long-lived executor: a bounded Tokio worker
//! pool driven by a supervisor thread. Trait calls spawn futures onto that
//! pool and wait on a standard channel for the result. This preserves the JJ
//! backend contract, remains safe when called from a tonic Tokio worker, and
//! — critically — does not create an OS thread per query.

use std::sync::mpsc::sync_channel;
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};
use sqlx::migrate::Migrator;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Row};
use tokio::runtime::{Builder, Handle};
use tokio::sync::oneshot;

use crate::object_db::{
    ObjectDb, ObjectDbError, ObjectDbLockGuard, ObjectDbResult, ObjectId, ObjectKind, OpId,
    RecordMutation,
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");
const GC_ADVISORY_LOCK_KEY: i64 = 0x5343_4845_4d41_4855;

fn publication_lock_key(repo: &str) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(b"schemahub-publication-lock-v1\0");
    hasher.update(repo.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    let key = i64::from_be_bytes(bytes);
    if key == GC_ADVISORY_LOCK_KEY {
        key ^ i64::MIN
    } else {
        key
    }
}

/// `sha256(tag ++ bytes)` — kind-tagged content hash. Identical to the redb
/// backend so any backend swap dedups against the same ids.
fn hash(kind: ObjectKind, bytes: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update([kind.tag()]);
    hasher.update(bytes);
    hasher.finalize().to_vec()
}

fn map_db<E: std::fmt::Display>(e: E) -> ObjectDbError {
    ObjectDbError::Backend(e.to_string())
}

/// Long-lived bridge from synchronous ObjectDb calls to SQLx futures.
struct PgExecutor {
    handle: Handle,
    shutdown: Option<oneshot::Sender<()>>,
    supervisor: Option<thread::JoinHandle<()>>,
    worker_threads: usize,
}

impl std::fmt::Debug for PgExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgExecutor")
            .field("worker_threads", &self.worker_threads)
            .finish_non_exhaustive()
    }
}

impl PgExecutor {
    fn new() -> ObjectDbResult<Self> {
        let worker_threads = thread::available_parallelism()
            .map(|parallelism| parallelism.get().clamp(2, 4))
            .unwrap_or(2);
        let (ready_tx, ready_rx) = sync_channel(1);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let supervisor = thread::Builder::new()
            .name("schemahub-pg-supervisor".to_string())
            .spawn(move || {
                let runtime = match Builder::new_multi_thread()
                    .worker_threads(worker_threads)
                    .enable_all()
                    .thread_name("schemahub-pg-io")
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if ready_tx.send(Ok(runtime.handle().clone())).is_err() {
                    return;
                }
                runtime.block_on(async {
                    let _ = shutdown_rx.await;
                });
                runtime.shutdown_timeout(Duration::from_secs(5));
            })
            .map_err(map_db)?;
        let handle = match ready_rx.recv() {
            Ok(Ok(handle)) => handle,
            Ok(Err(error)) => {
                let _ = supervisor.join();
                return Err(ObjectDbError::Backend(error));
            }
            Err(error) => {
                let _ = supervisor.join();
                return Err(map_db(error));
            }
        };
        Ok(Self {
            handle,
            shutdown: Some(shutdown_tx),
            supervisor: Some(supervisor),
            worker_threads,
        })
    }

    fn run<F, T>(&self, future: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        Self::run_on_handle(self.handle.clone(), future)
    }

    fn run_on_handle<F, T>(handle: Handle, future: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let (result_tx, result_rx) = sync_channel(1);
        handle.spawn(async move {
            let result = future.await;
            let _ = result_tx.send(result);
        });
        result_rx
            .recv()
            .expect("schemahub PostgreSQL executor task panicked")
    }
}

struct PgAdvisoryLockGuard {
    handle: Handle,
    connection: Option<PoolConnection<Postgres>>,
    key: i64,
    shared: bool,
}

impl std::fmt::Debug for PgAdvisoryLockGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgAdvisoryLockGuard")
            .field("key", &self.key)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

impl ObjectDbLockGuard for PgAdvisoryLockGuard {}

impl Drop for PgAdvisoryLockGuard {
    fn drop(&mut self) {
        let Some(mut connection) = self.connection.take() else {
            return;
        };
        let key = self.key;
        let shared = self.shared;
        PgExecutor::run_on_handle(self.handle.clone(), async move {
            let statement = if shared {
                "SELECT pg_advisory_unlock_shared($1)"
            } else {
                "SELECT pg_advisory_unlock($1)"
            };
            let _ = sqlx::query(statement)
                .bind(key)
                .execute(&mut *connection)
                .await;
        });
    }
}

impl Drop for PgExecutor {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(supervisor) = self.supervisor.take() {
            let _ = supervisor.join();
        }
    }
}

/// Postgres-backed object store. Sync façade over an async SQLx `PgPool`,
/// bridged through a fixed, long-lived executor owned by this instance.
pub struct PgObjectDb {
    pool: PgPool,
    lock_pool: PgPool,
    executor: PgExecutor,
}

impl std::fmt::Debug for PgObjectDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PgObjectDb").finish_non_exhaustive()
    }
}

impl PgObjectDb {
    /// Open a connection pool against `url` and ensure the schema exists.
    ///
    /// Sync wrapper: starts the dedicated executor, opens the pool on it, and
    /// creates the four tables idempotently.
    pub fn connect(url: &str) -> ObjectDbResult<Self> {
        let executor = PgExecutor::new()?;
        let url = url.to_string();
        let (pool, lock_pool) = executor
            .run(async move {
                let pool = PgPoolOptions::new()
                    .max_connections(8)
                    .connect(&url)
                    .await?;
                let lock_pool = PgPoolOptions::new()
                    .max_connections(16)
                    .connect(&url)
                    .await?;
                Ok::<_, sqlx::Error>((pool, lock_pool))
            })
            .map_err(map_db)?;
        let db = Self {
            pool,
            lock_pool,
            executor,
        };
        let pool = db.pool.clone();
        db.executor
            .run(async move { Self::init_schema(&pool).await })
            .map_err(map_db)?;
        Ok(db)
    }

    /// Construct a `PgObjectDb` over an externally-built `PgPool` (e.g. tests
    /// sharing one pool across many fixtures). Still owns its own executor for
    /// the sync↔async bridge.
    pub fn with_pool(pool: PgPool) -> ObjectDbResult<Self> {
        let executor = PgExecutor::new()?;
        let connection_options = pool.connect_options().as_ref().clone();
        let lock_pool = executor
            .run(async move {
                PgPoolOptions::new()
                    .max_connections(16)
                    .connect_with(connection_options)
                    .await
            })
            .map_err(map_db)?;
        let db = Self {
            pool,
            lock_pool,
            executor,
        };
        let pool = db.pool.clone();
        db.executor
            .run(async move { Self::init_schema(&pool).await })
            .map_err(map_db)?;
        Ok(db)
    }

    /// Schedule work on the fixed executor and synchronously await its result.
    fn block_on<F, T>(&self, fut: F) -> T
    where
        F: std::future::Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.executor.run(fut)
    }

    /// Apply all embedded, checksum-verified migrations. The baseline uses
    /// adoption-safe `IF NOT EXISTS` statements so pre-migration databases are
    /// enrolled without rewriting stored objects.
    async fn init_schema(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
        MIGRATOR.run(pool).await
    }

    fn acquire_advisory_guard(
        &self,
        key: i64,
        shared: bool,
    ) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        let lock_pool = self.lock_pool.clone();
        let connection = self
            .block_on(async move {
                let mut connection = lock_pool.acquire().await?;
                let statement = if shared {
                    "SELECT pg_advisory_lock_shared($1)"
                } else {
                    "SELECT pg_advisory_lock($1)"
                };
                sqlx::query(statement)
                    .bind(key)
                    .execute(&mut *connection)
                    .await?;
                Ok::<_, sqlx::Error>(connection)
            })
            .map_err(map_db)?;
        Ok(Box::new(PgAdvisoryLockGuard {
            handle: self.executor.handle.clone(),
            connection: Some(connection),
            key,
            shared,
        }))
    }
}

impl ObjectDb for PgObjectDb {
    fn acquire_mutation_guard(&self) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.acquire_advisory_guard(GC_ADVISORY_LOCK_KEY, true)
    }

    fn acquire_publication_guard(
        &self,
        repo: &str,
    ) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.acquire_advisory_guard(publication_lock_key(repo), false)
    }

    fn acquire_gc_guard(&self) -> ObjectDbResult<Box<dyn ObjectDbLockGuard + '_>> {
        self.acquire_advisory_guard(GC_ADVISORY_LOCK_KEY, false)
    }

    fn put_object(&self, kind: ObjectKind, bytes: &[u8]) -> ObjectDbResult<ObjectId> {
        let id = ObjectId(hash(kind, bytes));
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let value = bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO objects (kind, id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(kind_i)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(id)
    }

    fn put_object_at(&self, kind: ObjectKind, id: &ObjectId, bytes: &[u8]) -> ObjectDbResult<()> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let value = bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO objects (kind, id, bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(kind_i)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn get_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<Vec<u8>> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT bytes FROM objects WHERE kind = $1 AND id = $2")
                    .bind(kind_i)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(r.try_get::<Vec<u8>, _>("bytes").map_err(map_db)?),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn has_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<bool> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT 1 AS one FROM objects WHERE kind = $1 AND id = $2")
                    .bind(kind_i)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        Ok(row.is_some())
    }

    fn list_objects(&self, kind: ObjectKind) -> ObjectDbResult<Vec<ObjectId>> {
        let kind_i = kind.tag() as i32;
        let pool = self.pool.clone();
        let rows = self
            .block_on(async move {
                sqlx::query("SELECT id FROM objects WHERE kind = $1")
                    .bind(kind_i)
                    .fetch_all(&pool)
                    .await
            })
            .map_err(map_db)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(ObjectId(r.try_get::<Vec<u8>, _>("id").map_err(map_db)?));
        }
        Ok(out)
    }

    fn delete_object(&self, kind: ObjectKind, id: &ObjectId) -> ObjectDbResult<()> {
        let kind_i = kind.tag() as i32;
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query("DELETE FROM objects WHERE kind = $1 AND id = $2")
                .bind(kind_i)
                .bind(&id_bytes)
                .execute(&pool)
                .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn put_op(&self, repo: &str, op_bytes: &[u8]) -> ObjectDbResult<OpId> {
        // Op ids are content hashes of the operation record (matches redb/mem).
        let mut hasher = Sha256::new();
        hasher.update(b"op");
        hasher.update(op_bytes);
        let id = OpId(hasher.finalize().to_vec());
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let value = op_bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO ops (repo, op_id, op_bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, op_id) DO NOTHING",
            )
            .bind(&repo_s)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(id)
    }

    fn put_op_at(&self, repo: &str, id: &OpId, op_bytes: &[u8]) -> ObjectDbResult<()> {
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let value = op_bytes.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO ops (repo, op_id, op_bytes) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, op_id) DO NOTHING",
            )
            .bind(&repo_s)
            .bind(&id_bytes)
            .bind(&value)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn get_op(&self, repo: &str, id: &OpId) -> ObjectDbResult<Vec<u8>> {
        let repo_s = repo.to_string();
        let id_bytes = id.0.clone();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT op_bytes FROM ops WHERE repo = $1 AND op_id = $2")
                    .bind(&repo_s)
                    .bind(&id_bytes)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(r.try_get::<Vec<u8>, _>("op_bytes").map_err(map_db)?),
            None => Err(ObjectDbError::NotFound),
        }
    }

    fn list_ops(&self, repo: &str) -> ObjectDbResult<Vec<OpId>> {
        let repo_s = repo.to_string();
        let pool = self.pool.clone();
        let rows = self
            .block_on(async move {
                sqlx::query("SELECT op_id FROM ops WHERE repo = $1 ORDER BY inserted_at ASC")
                    .bind(&repo_s)
                    .fetch_all(&pool)
                    .await
            })
            .map_err(map_db)?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(OpId(r.try_get::<Vec<u8>, _>("op_id").map_err(map_db)?));
        }
        Ok(out)
    }

    fn list_repo_keys(&self) -> ObjectDbResult<Vec<String>> {
        let pool = self.pool.clone();
        self.block_on(async move {
            let rows = sqlx::query(
                "SELECT repo FROM ops
                 UNION
                 SELECT repo FROM refs
                 ORDER BY repo ASC",
            )
            .fetch_all(&pool)
            .await?;
            rows.into_iter()
                .map(|row| row.try_get::<String, _>("repo"))
                .collect::<Result<Vec<_>, sqlx::Error>>()
        })
        .map_err(map_db)
    }

    fn set_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<()> {
        let repo_s = repo.to_string();
        let name_s = name.to_string();
        let val = value.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            sqlx::query(
                "INSERT INTO refs (repo, name, target) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, name) DO UPDATE SET target = EXCLUDED.target",
            )
            .bind(&repo_s)
            .bind(&name_s)
            .bind(&val)
            .execute(&pool)
            .await
        })
        .map_err(map_db)?;
        Ok(())
    }

    fn create_ref(&self, repo: &str, name: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let repo = repo.to_string();
        let name = name.to_string();
        let value = value.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            let result = sqlx::query(
                "INSERT INTO refs (repo, name, target) VALUES ($1, $2, $3)
                 ON CONFLICT (repo, name) DO NOTHING",
            )
            .bind(&repo)
            .bind(&name)
            .bind(&value)
            .execute(&pool)
            .await?;
            Ok::<_, sqlx::Error>(result.rows_affected() == 1)
        })
        .map_err(map_db)
    }

    fn get_ref(&self, repo: &str, name: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let repo_s = repo.to_string();
        let name_s = name.to_string();
        let pool = self.pool.clone();
        let row = self
            .block_on(async move {
                sqlx::query("SELECT target FROM refs WHERE repo = $1 AND name = $2")
                    .bind(&repo_s)
                    .bind(&name_s)
                    .fetch_optional(&pool)
                    .await
            })
            .map_err(map_db)?;
        match row {
            Some(r) => Ok(Some(r.try_get::<Vec<u8>, _>("target").map_err(map_db)?)),
            None => Ok(None),
        }
    }

    fn create_record(&self, collection: &str, key: &str, value: &[u8]) -> ObjectDbResult<bool> {
        let collection = collection.to_string();
        let key = key.to_string();
        let value = value.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            let result = sqlx::query(
                "INSERT INTO resource_records (collection, record_key, record_bytes)
                 VALUES ($1, $2, $3)
                 ON CONFLICT (collection, record_key) DO NOTHING",
            )
            .bind(&collection)
            .bind(&key)
            .bind(&value)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(result.rows_affected() == 1)
        })
        .map_err(map_db)
    }

    fn create_records(&self, records: &[(&str, &str, &[u8])]) -> ObjectDbResult<bool> {
        let records: Vec<_> = records
            .iter()
            .map(|(collection, key, value)| {
                (
                    (*collection).to_string(),
                    (*key).to_string(),
                    (*value).to_vec(),
                )
            })
            .collect();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            for (collection, key, value) in records {
                let result = sqlx::query(
                    "INSERT INTO resource_records (collection, record_key, record_bytes)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (collection, record_key) DO NOTHING",
                )
                .bind(collection)
                .bind(key)
                .bind(value)
                .execute(&mut *tx)
                .await?;
                if result.rows_affected() != 1 {
                    tx.rollback().await?;
                    return Ok::<_, sqlx::Error>(false);
                }
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(true)
        })
        .map_err(map_db)
    }

    fn get_record(&self, collection: &str, key: &str) -> ObjectDbResult<Option<Vec<u8>>> {
        let collection = collection.to_string();
        let key = key.to_string();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            let row = sqlx::query(
                "SELECT record_bytes FROM resource_records
                 WHERE collection = $1 AND record_key = $2",
            )
            .bind(&collection)
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
            let value = row
                .map(|row| row.try_get::<Vec<u8>, _>("record_bytes"))
                .transpose()?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(value)
        })
        .map_err(map_db)
    }

    fn list_records(&self, collection: &str) -> ObjectDbResult<Vec<(String, Vec<u8>)>> {
        let collection = collection.to_string();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            let rows = sqlx::query(
                "SELECT record_key, record_bytes FROM resource_records
                 WHERE collection = $1 ORDER BY record_key ASC",
            )
            .bind(&collection)
            .fetch_all(&mut *tx)
            .await?;
            let records = rows
                .into_iter()
                .map(|row| {
                    Ok((
                        row.try_get::<String, _>("record_key")?,
                        row.try_get::<Vec<u8>, _>("record_bytes")?,
                    ))
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(records)
        })
        .map_err(map_db)
    }

    fn list_records_page(
        &self,
        collection: &str,
        start_after: Option<&str>,
        limit: usize,
    ) -> ObjectDbResult<Vec<(String, Vec<u8>)>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit)
            .map_err(|_| ObjectDbError::Backend("record page limit exceeds i64".to_string()))?;
        let collection = collection.to_string();
        let start_after = start_after.map(str::to_string);
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            // Keep the cursor predicate out of a nullable `OR`. Separate SQL
            // shapes let PostgreSQL use the `(collection, record_key)` primary
            // key as a true bounded range even after it selects a generic
            // prepared-statement plan.
            let rows = if let Some(start_after) = start_after.as_deref() {
                sqlx::query(
                    "SELECT record_key, record_bytes FROM resource_records
                     WHERE collection = $1 AND record_key > $2
                     ORDER BY record_key ASC
                     LIMIT $3",
                )
                .bind(&collection)
                .bind(start_after)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
            } else {
                sqlx::query(
                    "SELECT record_key, record_bytes FROM resource_records
                     WHERE collection = $1
                     ORDER BY record_key ASC
                     LIMIT $2",
                )
                .bind(&collection)
                .bind(limit)
                .fetch_all(&mut *tx)
                .await?
            };
            let records = rows
                .into_iter()
                .map(|row| {
                    Ok((
                        row.try_get::<String, _>("record_key")?,
                        row.try_get::<Vec<u8>, _>("record_bytes")?,
                    ))
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(records)
        })
        .map_err(map_db)
    }

    fn compare_and_swap_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
        replacement: &[u8],
    ) -> ObjectDbResult<bool> {
        let collection = collection.to_string();
        let key = key.to_string();
        let expected = expected.to_vec();
        let replacement = replacement.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            let result = sqlx::query(
                "UPDATE resource_records SET record_bytes = $4
                 WHERE collection = $1 AND record_key = $2 AND record_bytes = $3",
            )
            .bind(&collection)
            .bind(&key)
            .bind(&expected)
            .bind(&replacement)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(result.rows_affected() == 1)
        })
        .map_err(map_db)
    }

    fn compare_and_delete_record(
        &self,
        collection: &str,
        key: &str,
        expected: &[u8],
    ) -> ObjectDbResult<bool> {
        let collection = collection.to_string();
        let key = key.to_string();
        let expected = expected.to_vec();
        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            let result = sqlx::query(
                "DELETE FROM resource_records
                 WHERE collection = $1 AND record_key = $2 AND record_bytes = $3",
            )
            .bind(&collection)
            .bind(&key)
            .bind(&expected)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok::<_, sqlx::Error>(result.rows_affected() == 1)
        })
        .map_err(map_db)
    }

    fn transact_records(&self, mutations: &[RecordMutation<'_>]) -> ObjectDbResult<bool> {
        #[derive(Debug)]
        enum OwnedMutation {
            Create {
                collection: String,
                key: String,
                value: Vec<u8>,
            },
            CompareAndSwap {
                collection: String,
                key: String,
                expected: Vec<u8>,
                replacement: Vec<u8>,
            },
            CompareAndDelete {
                collection: String,
                key: String,
                expected: Vec<u8>,
            },
        }

        let mut keys = std::collections::HashSet::with_capacity(mutations.len());
        let mut owned = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let target = (
                mutation.collection().to_string(),
                mutation.key().to_string(),
            );
            if !keys.insert(target.clone()) {
                return Ok(false);
            }
            owned.push(match mutation {
                RecordMutation::Create { value, .. } => OwnedMutation::Create {
                    collection: target.0,
                    key: target.1,
                    value: value.to_vec(),
                },
                RecordMutation::CompareAndSwap {
                    expected,
                    replacement,
                    ..
                } => OwnedMutation::CompareAndSwap {
                    collection: target.0,
                    key: target.1,
                    expected: expected.to_vec(),
                    replacement: replacement.to_vec(),
                },
                RecordMutation::CompareAndDelete { expected, .. } => {
                    OwnedMutation::CompareAndDelete {
                        collection: target.0,
                        key: target.1,
                        expected: expected.to_vec(),
                    }
                }
            });
        }

        let pool = self.pool.clone();
        self.block_on(async move {
            let mut tx = pool.begin().await?;
            for mutation in owned {
                let affected = match mutation {
                    OwnedMutation::Create {
                        collection,
                        key,
                        value,
                    } => sqlx::query(
                        "INSERT INTO resource_records
                             (collection, record_key, record_bytes)
                             VALUES ($1, $2, $3)
                             ON CONFLICT (collection, record_key) DO NOTHING",
                    )
                    .bind(collection)
                    .bind(key)
                    .bind(value)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected(),
                    OwnedMutation::CompareAndSwap {
                        collection,
                        key,
                        expected,
                        replacement,
                    } => sqlx::query(
                        "UPDATE resource_records SET record_bytes = $4
                             WHERE collection = $1
                               AND record_key = $2
                               AND record_bytes = $3",
                    )
                    .bind(collection)
                    .bind(key)
                    .bind(expected)
                    .bind(replacement)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected(),
                    OwnedMutation::CompareAndDelete {
                        collection,
                        key,
                        expected,
                    } => sqlx::query(
                        "DELETE FROM resource_records
                             WHERE collection = $1
                               AND record_key = $2
                               AND record_bytes = $3",
                    )
                    .bind(collection)
                    .bind(key)
                    .bind(expected)
                    .execute(&mut *tx)
                    .await?
                    .rows_affected(),
                };
                if affected != 1 {
                    tx.rollback().await?;
                    return Ok::<_, sqlx::Error>(false);
                }
            }
            tx.commit().await?;
            Ok::<_, sqlx::Error>(true)
        })
        .map_err(map_db)
    }
}

#[cfg(test)]
mod executor_tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn executor_reuses_a_bounded_worker_pool_across_many_calls() {
        // Arrange
        let executor = PgExecutor::new().expect("create PostgreSQL executor");
        let worker_limit = executor.worker_threads;

        // Act
        let worker_ids: HashSet<_> = (0..128)
            .map(|_| executor.run(async { thread::current().id() }))
            .collect();

        // Assert
        assert!(!worker_ids.is_empty());
        assert!(worker_ids.len() <= worker_limit);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn executor_is_safe_to_call_from_an_existing_tokio_runtime() {
        // Arrange
        let executor = PgExecutor::new().expect("create PostgreSQL executor");

        // Act
        let value = executor.run(async { 42_u64 });

        // Assert
        assert_eq!(value, 42);
    }
}

// ── Integration tests ────────────────────────────────────────────────────────
//
// Gated by the `postgres-integration` feature so a plain `cargo test` (default
// features) does not require a Postgres instance. Run with:
//
//   SCHEMAHUB_TEST_POSTGRES_URL=postgres://cyuan:cyuan@docker.yuacx.com:5432/postgres \
//       cargo test -p schemahub-jj --features postgres-integration

#[cfg(all(test, feature = "postgres-integration"))]
mod tests {
    use super::*;
    use schemahub_types::{DeclBlob, MutationEffect};
    use sqlx::AssertSqlSafe;
    use std::sync::Arc;
    use tokio::runtime::Runtime;
    use uuid::Uuid;

    /// Fresh `PgObjectDb` bound to a UNIQUELY-named schema so parallel test
    /// runs don't collide. The schema is dropped on `Drop` so test runs leave
    /// no residue.
    struct TestDb {
        db: Arc<PgObjectDb>,
        schema: String,
        /// Admin pool used to drop the schema in `Drop` — sits on the public
        /// schema so it doesn't depend on the test schema we're about to drop.
        admin_pool: PgPool,
        admin_runtime: Arc<Runtime>,
    }

    impl TestDb {
        fn new() -> Self {
            Self::new_inner(false)
        }

        fn adopting_legacy_schema() -> Self {
            Self::new_inner(true)
        }

        fn new_inner(create_legacy_schema: bool) -> Self {
            let url = std::env::var("SCHEMAHUB_TEST_POSTGRES_URL").expect(
                "SCHEMAHUB_TEST_POSTGRES_URL must be set to run postgres-integration tests",
            );
            // A unique schema per test isolates all four tables
            // (objects/ops/refs/resource_records) and lets us drop them all in
            // one statement.
            let schema = format!("shvcs_test_{}", Uuid::new_v4().simple());

            // Admin runtime + pool (separate from the PgObjectDb's runtime/pool):
            // used to CREATE/DROP SCHEMA and to set up the search_path.
            let admin_runtime = Arc::new(
                Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .thread_name("schemahub-pg-test-admin")
                    .build()
                    .expect("admin runtime"),
            );
            let admin_pool = admin_runtime
                .block_on(async { PgPoolOptions::new().max_connections(2).connect(&url).await })
                .expect("connect admin pool");
            // SAFETY (SqlSafeStr): `schema` is `shvcs_test_<uuid hex>` — fully
            // controlled by this test, ASCII alphanumeric + underscore, no
            // user input.
            let create_sql = format!("CREATE SCHEMA \"{schema}\"");
            admin_runtime
                .block_on(async {
                    sqlx::query(AssertSqlSafe(create_sql))
                        .execute(&admin_pool)
                        .await
                })
                .expect("create schema");

            // The PgObjectDb pool needs `search_path` to point at our schema so
            // its CREATE TABLE IF NOT EXISTS lands inside it. sqlx's PgPoolOptions
            // `after_connect` hook is the clean way to do that on every checkout.
            let schema_for_hook = schema.clone();
            let pool = admin_runtime
                .block_on(async {
                    PgPoolOptions::new()
                        .max_connections(4)
                        .after_connect(move |conn, _meta| {
                            let s = schema_for_hook.clone();
                            Box::pin(async move {
                                // SAFETY (SqlSafeStr): same as above — `s` is the
                                // test-controlled schema identifier.
                                let sql = format!("SET search_path TO \"{s}\"");
                                sqlx::query(AssertSqlSafe(sql))
                                    .execute(&mut *conn)
                                    .await
                                    .map(|_| ())
                            })
                        })
                        .connect(&url)
                        .await
                })
                .expect("connect test pool");
            if create_legacy_schema {
                admin_runtime
                    .block_on(async {
                        sqlx::raw_sql(include_str!(
                            "../migrations/202607210001_initial_schema.sql"
                        ))
                        .execute(&pool)
                        .await?;
                        sqlx::query(
                            "INSERT INTO resource_records
                             (collection, record_key, record_bytes) VALUES ($1, $2, $3)",
                        )
                        .bind("legacy")
                        .bind("sentinel")
                        .bind(b"preserved".as_slice())
                        .execute(&pool)
                        .await?;
                        Ok::<_, sqlx::Error>(())
                    })
                    .expect("create legacy schema");
            }
            let db = Arc::new(PgObjectDb::with_pool(pool).expect("initialize test database"));

            Self {
                db,
                schema,
                admin_pool,
                admin_runtime,
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            // SAFETY (SqlSafeStr): test-controlled schema identifier.
            let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema);
            let _ = self.admin_runtime.block_on(async {
                sqlx::query(AssertSqlSafe(sql))
                    .execute(&self.admin_pool)
                    .await
            });
        }
    }

    #[test]
    fn embedded_migration_is_versioned_and_idempotent() {
        // Arrange
        let t = TestDb::new();
        let pool = t.db.pool.clone();

        // Act
        let versions = t.db.block_on(async move {
            MIGRATOR.run(&pool).await.expect("rerun migrations");
            sqlx::query_scalar::<_, i64>(
                "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .expect("list migration versions")
        });

        // Assert
        assert_eq!(versions, vec![202_607_210_001]);
    }

    #[test]
    fn baseline_migration_adopts_legacy_tables_without_rewriting_data() {
        // Arrange
        let t = TestDb::adopting_legacy_schema();

        // Act
        let sentinel =
            t.db.get_record("legacy", "sentinel")
                .expect("read adopted record");
        let pool = t.db.pool.clone();
        let versions = t.db.block_on(async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT version FROM _sqlx_migrations WHERE success ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .expect("list adopted migration versions")
        });

        // Assert
        assert_eq!(sentinel, Some(b"preserved".to_vec()));
        assert_eq!(versions, vec![202_607_210_001]);
    }

    // ── ObjectDb contract — objects ──────────────────────────────────────────

    #[test]
    fn object_roundtrip_returns_identical_bytes() {
        // Arrange
        let t = TestDb::new();
        let bytes = b"hello declaration blob";

        // Act
        let id = t.db.put_object(ObjectKind::File, bytes).unwrap();
        let fetched = t.db.get_object(ObjectKind::File, &id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
        assert!(t.db.has_object(ObjectKind::File, &id).unwrap());
    }

    #[test]
    fn put_object_is_content_addressed_and_dedups() {
        // Arrange
        let t = TestDb::new();
        let bytes = b"same content";

        // Act
        let id1 = t.db.put_object(ObjectKind::File, bytes).unwrap();
        let id2 = t.db.put_object(ObjectKind::File, bytes).unwrap();

        // Assert
        assert_eq!(id1, id2);
        assert_eq!(t.db.list_objects(ObjectKind::File).unwrap().len(), 1);
    }

    #[test]
    fn get_object_missing_returns_not_found() {
        // Arrange
        let t = TestDb::new();
        let bogus = ObjectId(vec![0u8; 32]);

        // Act
        let result = t.db.get_object(ObjectKind::File, &bogus);

        // Assert
        assert!(matches!(result, Err(ObjectDbError::NotFound)));
    }

    #[test]
    fn list_objects_filters_by_kind() {
        // Arrange
        let t = TestDb::new();
        let file_id = t.db.put_object(ObjectKind::File, b"file blob").unwrap();
        let tree_id = t.db.put_object(ObjectKind::Tree, b"tree blob").unwrap();

        // Act
        let files = t.db.list_objects(ObjectKind::File).unwrap();
        let trees = t.db.list_objects(ObjectKind::Tree).unwrap();

        // Assert
        assert_eq!(files, vec![file_id]);
        assert_eq!(trees, vec![tree_id]);
    }

    #[test]
    fn delete_object_removes_it_and_is_idempotent() {
        // Arrange
        let t = TestDb::new();
        let id = t.db.put_object(ObjectKind::File, b"transient").unwrap();
        assert!(t.db.has_object(ObjectKind::File, &id).unwrap());

        // Act
        t.db.delete_object(ObjectKind::File, &id).unwrap();
        // Second delete must be a no-op.
        t.db.delete_object(ObjectKind::File, &id).unwrap();

        // Assert
        assert!(!t.db.has_object(ObjectKind::File, &id).unwrap());
    }

    #[test]
    fn put_object_at_writes_under_caller_id() {
        // Arrange
        let t = TestDb::new();
        let caller_id = ObjectId(vec![42u8; 32]);
        let bytes = b"jj-owned blob";

        // Act
        t.db.put_object_at(ObjectKind::Tree, &caller_id, bytes)
            .unwrap();
        let fetched = t.db.get_object(ObjectKind::Tree, &caller_id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
    }

    // ── ObjectDb contract — ops ──────────────────────────────────────────────

    #[test]
    fn put_op_then_get_op_roundtrips() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";
        let bytes = b"op record bytes";

        // Act
        let id = t.db.put_op(repo, bytes).unwrap();
        let fetched = t.db.get_op(repo, &id).unwrap();

        // Assert
        assert_eq!(fetched, bytes);
    }

    #[test]
    fn list_ops_returns_inserts_in_oldest_to_newest_order() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";

        // Act — three ops with distinct contents, inserted in order.
        let a = t.db.put_op(repo, b"op-a").unwrap();
        let b = t.db.put_op(repo, b"op-b").unwrap();
        let c = t.db.put_op(repo, b"op-c").unwrap();
        let ids = t.db.list_ops(repo).unwrap();

        // Assert — Postgres `inserted_at BIGSERIAL` preserves insertion order
        // for a single writer, the contract `Jj::list_operations` relies on.
        assert_eq!(ids, vec![a, b, c]);
    }

    #[test]
    fn ops_are_scoped_per_repo() {
        // Arrange
        let t = TestDb::new();
        let _ = t.db.put_op("proj/one", b"op-1").unwrap();
        let _ = t.db.put_op("proj/two", b"op-2").unwrap();

        // Act
        let one = t.db.list_ops("proj/one").unwrap();
        let two = t.db.list_ops("proj/two").unwrap();

        // Assert
        assert_eq!(one.len(), 1);
        assert_eq!(two.len(), 1);
        assert_ne!(one, two);
    }

    #[test]
    fn repository_inventory_unions_operation_and_ref_scopes() {
        // Arrange
        let t = TestDb::new();
        t.db.put_op("alpha/schemas", b"alpha-op").unwrap();
        t.db.set_ref("beta/schemas", "op-head", b"beta-ref")
            .unwrap();
        t.db.put_op("beta/schemas", b"beta-op").unwrap();

        // Act
        let repos = t.db.list_repo_keys().unwrap();

        // Assert
        assert_eq!(
            repos,
            vec!["alpha/schemas".to_string(), "beta/schemas".to_string()]
        );
    }

    #[test]
    fn distributed_gc_guard_waits_for_active_mutations() {
        // Arrange
        let t = TestDb::new();
        let second_instance =
            PgObjectDb::with_pool(t.db.pool.clone()).expect("open second database instance");
        let mutation_guard =
            t.db.acquire_mutation_guard()
                .expect("acquire shared mutation guard");
        let (acquired_tx, acquired_rx) = std::sync::mpsc::sync_channel(1);

        // Act
        let (before_release, after_release) = thread::scope(|scope| {
            scope.spawn(move || {
                let _gc_guard = second_instance
                    .acquire_gc_guard()
                    .expect("acquire exclusive GC guard");
                acquired_tx.send(()).expect("report GC acquisition");
            });
            let before_release = acquired_rx.recv_timeout(Duration::from_millis(200));
            drop(mutation_guard);
            let after_release = acquired_rx.recv_timeout(Duration::from_secs(5));
            (before_release, after_release)
        });

        // Assert
        assert_eq!(
            before_release,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        assert_eq!(after_release, Ok(()));
    }

    #[test]
    fn distributed_publication_guard_is_exclusive_per_repository() {
        // Arrange
        let t = TestDb::new();
        let same_repo_instance =
            PgObjectDb::with_pool(t.db.pool.clone()).expect("open same-repo instance");
        let other_repo_instance =
            PgObjectDb::with_pool(t.db.pool.clone()).expect("open other-repo instance");
        let first =
            t.db.acquire_publication_guard("acme/core")
                .expect("acquire first publication guard");
        let (acquired_tx, acquired_rx) = std::sync::mpsc::sync_channel(1);

        // Act
        let (same_before_release, other_repo, same_after_release) = thread::scope(|scope| {
            scope.spawn(move || {
                let _guard = same_repo_instance
                    .acquire_publication_guard("acme/core")
                    .expect("acquire same-repo publication guard");
                acquired_tx.send(()).expect("report same-repo acquisition");
            });
            let same_before_release = acquired_rx.recv_timeout(Duration::from_millis(200));
            let other_repo = other_repo_instance
                .acquire_publication_guard("acme/other")
                .is_ok();
            drop(first);
            let same_after_release = acquired_rx.recv_timeout(Duration::from_secs(5));
            (same_before_release, other_repo, same_after_release)
        });

        // Assert
        assert_eq!(
            same_before_release,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        );
        assert!(other_repo);
        assert_eq!(same_after_release, Ok(()));
    }

    #[test]
    fn postgres_gc_restart_drill_preserves_cross_repo_history_and_undo() {
        // Arrange
        let t = TestDb::new();
        let jj = crate::Jj::new(t.db.clone());
        let effect = |name: &str, value: &str| MutationEffect {
            meta: None,
            upserts: vec![(name.to_string(), DeclBlob::new(value.as_bytes().to_vec()))],
            removes: Vec::new(),
        };
        jj.commit_write(
            "alpha",
            "schemas",
            "main",
            "alpha.proto",
            &crate::RefSpec::bookmark("main"),
            effect("Alpha", "alpha-v1"),
            "alice",
            "seed alpha",
        )
        .unwrap();
        let alpha_v1 = jj
            .get_declaration(
                "alpha",
                "schemas",
                "alpha.proto",
                "Alpha",
                &crate::RefSpec::bookmark("main"),
            )
            .unwrap();
        jj.commit_write(
            "alpha",
            "schemas",
            "main",
            "alpha.proto",
            &crate::RefSpec::bookmark("main"),
            effect("Alpha", "alpha-v2"),
            "alice",
            "update alpha",
        )
        .unwrap();
        jj.commit_write(
            "beta",
            "schemas",
            "main",
            "beta.proto",
            &crate::RefSpec::bookmark("main"),
            effect("Beta", "beta-live"),
            "bob",
            "seed beta",
        )
        .unwrap();
        t.db.put_object(ObjectKind::File, b"postgres orphan before GC")
            .unwrap();

        // Act
        let swept = jj
            .gc(&[("alpha".to_string(), "schemas".to_string())])
            .unwrap();
        drop(jj);
        let restarted_db = Arc::new(
            PgObjectDb::with_pool(t.db.pool.clone()).expect("reopen PostgreSQL object database"),
        );
        let restarted = crate::Jj::new(restarted_db);
        let beta_after_restart = restarted
            .get_declaration(
                "beta",
                "schemas",
                "beta.proto",
                "Beta",
                &crate::RefSpec::bookmark("main"),
            )
            .unwrap();
        restarted.undo("alpha", "schemas", "operator").unwrap();
        let alpha_after_undo = restarted
            .get_declaration(
                "alpha",
                "schemas",
                "alpha.proto",
                "Alpha",
                &crate::RefSpec::bookmark("main"),
            )
            .unwrap();

        // Assert
        assert!(swept >= 1);
        assert_eq!(beta_after_restart, DeclBlob::new(b"beta-live".to_vec()));
        assert_eq!(alpha_after_undo, alpha_v1);
    }

    // ── ObjectDb contract — refs ─────────────────────────────────────────────

    #[test]
    fn refs_set_and_get_roundtrip() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";

        // Act
        t.db.set_ref(repo, "HEAD", b"deadbeef").unwrap();
        let value = t.db.get_ref(repo, "HEAD").unwrap();

        // Assert
        assert_eq!(value, Some(b"deadbeef".to_vec()));
    }

    #[test]
    fn refs_set_overwrites_previous_value() {
        // Arrange
        let t = TestDb::new();
        let repo = "proj/repo";
        t.db.set_ref(repo, "HEAD", b"v1").unwrap();

        // Act
        t.db.set_ref(repo, "HEAD", b"v2").unwrap();

        // Assert
        assert_eq!(t.db.get_ref(repo, "HEAD").unwrap(), Some(b"v2".to_vec()));
    }

    #[test]
    fn get_ref_missing_returns_none() {
        // Arrange
        let t = TestDb::new();

        // Act
        let value = t.db.get_ref("proj/repo", "no-such-ref").unwrap();

        // Assert
        assert_eq!(value, None);
    }

    #[test]
    fn create_ref_is_atomic_and_does_not_overwrite() {
        // Arrange
        let t = TestDb::new();

        // Act
        let first =
            t.db.create_ref("proj/repo", "op_heads", b"root")
                .expect("create ref");
        let second =
            t.db.create_ref("proj/repo", "op_heads", b"other")
                .expect("repeat create");

        // Assert
        assert!(first);
        assert!(!second);
        assert_eq!(
            t.db.get_ref("proj/repo", "op_heads").expect("read ref"),
            Some(b"root".to_vec())
        );
    }

    // ── ObjectDb contract — mutable resource records ────────────────────────

    #[test]
    fn resource_record_create_get_and_list_are_collection_scoped() {
        // Arrange
        let t = TestDb::new();

        // Act
        let first =
            t.db.create_record("changes", "change-b", b"record-b")
                .unwrap();
        let second =
            t.db.create_record("changes", "change-a", b"record-a")
                .unwrap();
        let duplicate =
            t.db.create_record("changes", "change-a", b"replacement")
                .unwrap();
        t.db.create_record("repos", "change-a", b"other collection")
            .unwrap();
        let fetched = t.db.get_record("changes", "change-a").unwrap();
        let listed = t.db.list_records("changes").unwrap();

        // Assert
        assert!(first);
        assert!(second);
        assert!(!duplicate);
        assert_eq!(fetched, Some(b"record-a".to_vec()));
        assert_eq!(
            listed,
            vec![
                ("change-a".to_string(), b"record-a".to_vec()),
                ("change-b".to_string(), b"record-b".to_vec()),
            ]
        );
    }

    #[test]
    fn resource_record_pages_are_bounded_stable_and_collection_scoped() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("audit", "c", b"third").unwrap();
        t.db.create_record("audit", "a", b"first").unwrap();
        t.db.create_record("audit", "b", b"second").unwrap();
        t.db.create_record("other", "aa", b"outside").unwrap();

        // Act
        let first = t.db.list_records_page("audit", None, 2).unwrap();
        let second = t.db.list_records_page("audit", Some("b"), 2).unwrap();
        let empty = t.db.list_records_page("audit", None, 0).unwrap();

        // Assert
        assert_eq!(
            first,
            vec![
                ("a".to_string(), b"first".to_vec()),
                ("b".to_string(), b"second".to_vec()),
            ]
        );
        assert_eq!(second, vec![("c".to_string(), b"third".to_vec())]);
        assert!(empty.is_empty());
    }

    #[test]
    fn resource_record_compare_and_swap_rejects_stale_bytes_atomically() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("changes", "change-a", b"v1").unwrap();

        // Act
        let replaced =
            t.db.compare_and_swap_record("changes", "change-a", b"v1", b"v2")
                .unwrap();
        let stale =
            t.db.compare_and_swap_record("changes", "change-a", b"v1", b"v3")
                .unwrap();
        let current = t.db.get_record("changes", "change-a").unwrap();

        // Assert
        assert!(replaced);
        assert!(!stale);
        assert_eq!(current, Some(b"v2".to_vec()));
    }

    #[test]
    fn resource_record_compare_and_delete_rejects_stale_bytes_atomically() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("idempotency", "receipt-a", b"v1")
            .unwrap();

        // Act
        let stale =
            t.db.compare_and_delete_record("idempotency", "receipt-a", b"stale")
                .unwrap();
        let deleted =
            t.db.compare_and_delete_record("idempotency", "receipt-a", b"v1")
                .unwrap();
        let current = t.db.get_record("idempotency", "receipt-a").unwrap();

        // Assert
        assert!(!stale);
        assert!(deleted);
        assert_eq!(current, None);
    }

    #[test]
    fn resource_record_batch_create_is_all_or_nothing() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("roles", "acme/alice", b"owner").unwrap();
        let conflicting = [
            ("projects", "acme", b"project".as_slice()),
            ("roles", "acme/alice", b"owner".as_slice()),
        ];
        let clean = [
            ("projects", "commerce", b"project".as_slice()),
            ("roles", "commerce/alice", b"owner".as_slice()),
        ];

        // Act
        let rejected = t.db.create_records(&conflicting).unwrap();
        let inserted = t.db.create_records(&clean).unwrap();

        // Assert
        assert!(!rejected);
        assert_eq!(t.db.get_record("projects", "acme").unwrap(), None);
        assert!(inserted);
        assert_eq!(
            t.db.get_record("projects", "commerce").unwrap(),
            Some(b"project".to_vec())
        );
        assert_eq!(
            t.db.get_record("roles", "commerce/alice").unwrap(),
            Some(b"owner".to_vec())
        );
    }

    #[test]
    fn resource_record_transaction_rejection_writes_nothing() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("resources", "acme", b"v1").unwrap();
        let mutations = [
            RecordMutation::CompareAndSwap {
                collection: "resources",
                key: "acme",
                expected: b"stale",
                replacement: b"v2",
            },
            RecordMutation::Create {
                collection: "audit",
                key: "event-a",
                value: b"created",
            },
        ];

        // Act
        let committed = t.db.transact_records(&mutations).unwrap();

        // Assert
        assert!(!committed);
        assert_eq!(
            t.db.get_record("resources", "acme").unwrap(),
            Some(b"v1".to_vec())
        );
        assert_eq!(t.db.get_record("audit", "event-a").unwrap(), None);
    }

    #[test]
    fn resource_record_transaction_success_writes_everything() {
        // Arrange
        let t = TestDb::new();
        t.db.create_record("resources", "acme", b"v1").unwrap();
        let mutations = [
            RecordMutation::CompareAndSwap {
                collection: "resources",
                key: "acme",
                expected: b"v1",
                replacement: b"v2",
            },
            RecordMutation::Create {
                collection: "audit",
                key: "event-a",
                value: b"created",
            },
        ];

        // Act
        let committed = t.db.transact_records(&mutations).unwrap();

        // Assert
        assert!(committed);
        assert_eq!(
            t.db.get_record("resources", "acme").unwrap(),
            Some(b"v2".to_vec())
        );
        assert_eq!(
            t.db.get_record("audit", "event-a").unwrap(),
            Some(b"created".to_vec())
        );
    }

    #[test]
    fn concurrent_resource_writers_complete_without_thread_per_query_execution() {
        // Arrange
        const WRITERS: usize = 16;
        const RECORDS_PER_WRITER: usize = 50;
        let t = TestDb::new();
        let start = std::sync::Barrier::new(WRITERS);

        // Act
        thread::scope(|scope| {
            let start = &start;
            let db = &t.db;
            for writer in 0..WRITERS {
                scope.spawn(move || {
                    start.wait();
                    for record in 0..RECORDS_PER_WRITER {
                        let key = format!("writer-{writer:02}/record-{record:03}");
                        let value = format!("value-{writer}-{record}");
                        assert!(db
                            .create_record("concurrency-load", &key, value.as_bytes())
                            .expect("create concurrent record"));
                    }
                });
            }
        });
        let records =
            t.db.list_records("concurrency-load")
                .expect("list concurrent records");

        // Assert
        assert_eq!(records.len(), WRITERS * RECORDS_PER_WRITER);
        assert_eq!(
            records.first().map(|(key, _)| key.as_str()),
            Some("writer-00/record-000")
        );
        assert_eq!(
            records.last().map(|(key, _)| key.as_str()),
            Some("writer-15/record-049")
        );
    }

    #[test]
    fn concurrent_compare_and_swap_retries_preserve_every_increment() {
        // Arrange
        const WRITERS: usize = 8;
        const INCREMENTS_PER_WRITER: usize = 25;
        let t = TestDb::new();
        assert!(t
            .db
            .create_record("cas-load", "counter", b"0")
            .expect("create counter"));
        let start = std::sync::Barrier::new(WRITERS);

        // Act
        thread::scope(|scope| {
            let start = &start;
            let db = &t.db;
            for _ in 0..WRITERS {
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..INCREMENTS_PER_WRITER {
                        loop {
                            let current = db
                                .get_record("cas-load", "counter")
                                .expect("read counter")
                                .expect("counter exists");
                            let current_value: usize = std::str::from_utf8(&current)
                                .expect("UTF-8 counter")
                                .parse()
                                .expect("numeric counter");
                            let replacement = (current_value + 1).to_string();
                            if db
                                .compare_and_swap_record(
                                    "cas-load",
                                    "counter",
                                    &current,
                                    replacement.as_bytes(),
                                )
                                .expect("compare and swap counter")
                            {
                                break;
                            }
                        }
                    }
                });
            }
        });
        let final_value =
            t.db.get_record("cas-load", "counter")
                .expect("read final counter")
                .expect("counter exists");

        // Assert
        assert_eq!(
            std::str::from_utf8(&final_value).expect("UTF-8 final counter"),
            (WRITERS * INCREMENTS_PER_WRITER).to_string()
        );
    }
}
