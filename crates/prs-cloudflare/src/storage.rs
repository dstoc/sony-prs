//! Worker-owned storage for the single current PRSync bundle.
//!
//! R2 objects are never exposed as a binding to a client. The Worker uses the
//! binding below to validate, replace, and publish bundle objects.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::Cursor;

use prs_sync_bundle::validate;
use prs_sync_protocol::{EntityTag, InboxRevision, Manifest, MAX_BUNDLE_SIZE};
use serde::Deserialize;
use worker::{wasm_bindgen::JsCast, Bucket, D1Database, Error, Object, Result};

/// Bundle objects are private Worker storage. Cleanup lists this prefix so it
/// can also find legacy objects that predate candidate lifecycle metadata.
pub(crate) const BUNDLE_OBJECT_PREFIX: &str = "bundles/";

/// New objects use this prefix to distinguish them from legacy object keys.
pub(crate) const CANDIDATE_OBJECT_PREFIX: &str = "bundles/candidates/";

/// An abandoned object or metadata row is retained for 24 hours. This gives a
/// failed request time to retry while keeping cleanup eventually effective.
pub(crate) const CLEANUP_RETENTION_SECONDS: u64 = 24 * 60 * 60;

const CLEANUP_BATCH_SIZE: u32 = 100;
const CLEANUP_MAX_R2_OBJECTS: u32 = 100;

const CANDIDATE_STATE_METADATA: &str = "candidate";

/// The Worker-owned D1 and R2 handles for bundle publication.
#[derive(Debug)]
pub(crate) struct BundleStore {
    database: D1Database,
    bucket: Bucket,
}

/// Errors from a bundle replacement, separated by client input and Worker
/// storage or publication failures.
pub(crate) enum BundlePushError {
    InvalidBundle(Error),
    Storage(Error),
}

impl From<Error> for BundlePushError {
    fn from(error: Error) -> Self {
        Self::Storage(error)
    }
}

impl BundleStore {
    pub(crate) fn from_env(database: D1Database, bucket: Bucket) -> Self {
        Self { database, bucket }
    }

    /// Replaces the current bundle using the destructive replacement policy.
    ///
    /// The inbox is cleared before validation and storage. The final D1 batch
    /// inserts immutable metadata and publishes the reference together, so an
    /// invalid bundle or failed write cannot become current.
    pub(crate) async fn push(
        &self,
        bytes: Vec<u8>,
        updated_at: u64,
    ) -> std::result::Result<PublishedBundle, BundlePushError> {
        let previous = self.current_bundle().await?;
        self.clear_inbox(updated_at).await?;
        if let Some(previous) = previous {
            self.bucket.delete(previous.object_key).await?;
        }

        let manifest = validate_for_publication(&bytes).map_err(BundlePushError::InvalidBundle)?;

        let bundle_id = candidate_id()?;
        let object_key = candidate_object_key(&bundle_id);
        self.reserve_lifecycle(&bundle_id, &object_key, updated_at)
            .await?;
        let object = self
            .bucket
            .put(&object_key, bytes)
            .custom_metadata(HashMap::from([
                ("prs-bundle-id".to_owned(), bundle_id.clone()),
                (
                    "prs-publication-state".to_owned(),
                    CANDIDATE_STATE_METADATA.to_owned(),
                ),
            ]))
            .execute()
            .await?
            .ok_or_else(|| Error::RustError("R2 did not return the stored object".into()))?;

        let etag = object.etag();
        let size_bytes = object.size();
        let manifest_json = serde_json::to_string(&manifest).map_err(|error| {
            BundlePushError::Storage(Error::RustError(format!(
                "failed to encode bundle manifest: {error}"
            )))
        })?;

        let revision = self
            .publish(
                &bundle_id,
                &object_key,
                &etag,
                &manifest_json,
                size_bytes,
                updated_at,
            )
            .await?;

        Ok(PublishedBundle {
            bundle_id,
            object_key,
            revision,
            etag,
            manifest,
            size_bytes,
        })
    }

    /// Clears the current reference and removes its current R2 object.
    ///
    /// Clearing D1 first prevents a failed R2 delete from leaving a reference
    /// to an object that the reader cannot retrieve. The old object is then an
    /// unreferenced cleanup candidate if deletion fails.
    pub(crate) async fn clear(&self, updated_at: u64) -> Result<InboxRevision> {
        let previous = self.current_bundle().await?;
        let revision = self.clear_inbox(updated_at).await?;
        if let Some(previous) = previous {
            self.bucket.delete(previous.object_key).await?;
        }
        Ok(revision)
    }

    /// Reads only the current inbox metadata. The returned object key is an
    /// internal value used by the reader route to fetch the private R2 object.
    pub(crate) async fn current(&self) -> Result<CurrentBundle> {
        let inbox: CurrentInboxRow = self
            .database
            .prepare(
                "SELECT revision, current_bundle_id
                 FROM inbox
                 WHERE singleton = 1",
            )
            .first(None)
            .await?
            .ok_or_else(|| Error::RustError("inbox singleton is missing".into()))?;
        let revision = revision(inbox.revision)?;

        let Some(bundle_id) = inbox.current_bundle_id else {
            return Ok(CurrentBundle {
                revision,
                etag: None,
                manifest: None,
                object_key: None,
                size_bytes: 0,
            });
        };

        let bundle: BundleMetadataRow = self
            .database
            .prepare(
                "SELECT object_key, etag, manifest_json, size_bytes
                 FROM bundles
                 WHERE bundle_id = ?",
            )
            .bind(&[worker::wasm_bindgen::JsValue::from_str(&bundle_id)])?
            .first(None)
            .await?
            .ok_or_else(|| {
                Error::RustError(format!(
                    "inbox references missing bundle metadata for {bundle_id}"
                ))
            })?;
        let manifest = serde_json::from_str(&bundle.manifest_json)
            .map_err(|error| Error::RustError(format!("stored manifest is invalid: {error}")))?;
        let etag =
            EntityTag::new(bundle.etag).map_err(|error| Error::RustError(error.to_string()))?;

        Ok(CurrentBundle {
            revision,
            etag: Some(etag),
            manifest: Some(manifest),
            object_key: Some(bundle.object_key),
            size_bytes: u64::try_from(bundle.size_bytes)
                .map_err(|_| Error::RustError("stored bundle size is negative".into()))?,
        })
    }

    /// Opens the current bundle body for a reader. No route receives the R2
    /// binding or an object key supplied by the caller.
    pub(crate) async fn current_object(&self, current: &CurrentBundle) -> Result<Option<Object>> {
        let Some(object_key) = current.object_key.clone() else {
            return Ok(None);
        };
        self.bucket.get(object_key).execute().await
    }

    async fn current_bundle(&self) -> Result<Option<StoredBundle>> {
        let inbox: InboxRow = self
            .database
            .prepare("SELECT current_bundle_id FROM inbox WHERE singleton = 1")
            .first(None)
            .await?
            .ok_or_else(|| Error::RustError("inbox singleton is missing".into()))?;

        let Some(bundle_id) = inbox.current_bundle_id else {
            return Ok(None);
        };

        let bundle: BundleRow = self
            .database
            .prepare("SELECT object_key FROM bundles WHERE bundle_id = ?")
            .bind(&[worker::wasm_bindgen::JsValue::from_str(&bundle_id)])?
            .first(None)
            .await?
            .ok_or_else(|| {
                Error::RustError(format!(
                    "inbox references missing bundle metadata for {bundle_id}"
                ))
            })?;

        Ok(Some(StoredBundle {
            object_key: bundle.object_key,
        }))
    }

    async fn clear_inbox(&self, updated_at: u64) -> Result<InboxRevision> {
        let row: Option<RevisionRow> = self
            .database
            .prepare(
                "UPDATE inbox
                 SET current_bundle_id = NULL, revision = revision + 1, updated_at = ?
                 WHERE singleton = 1
                 RETURNING revision",
            )
            .bind(&[number(updated_at)])?
            .first(None)
            .await?;
        row.map(|row| revision(row.revision))
            .ok_or_else(|| Error::RustError("inbox singleton is missing".into()))?
    }

    async fn publish(
        &self,
        bundle_id: &str,
        object_key: &str,
        etag: &str,
        manifest_json: &str,
        size_bytes: u64,
        created_at: u64,
    ) -> Result<InboxRevision> {
        let insert_bundle = self
            .database
            .prepare(
                "INSERT INTO bundles
                    (bundle_id, object_key, etag, manifest_json, size_bytes, created_at, lifecycle_id)
                 SELECT ?, ?, ?, ?, ?, ?, lifecycle_id
                 FROM bundle_lifecycle
                 WHERE lifecycle_id = ? AND state = 'uploading'",
            )
            .bind(&[
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
                worker::wasm_bindgen::JsValue::from_str(object_key),
                worker::wasm_bindgen::JsValue::from_str(etag),
                worker::wasm_bindgen::JsValue::from_str(manifest_json),
                number(size_bytes),
                number(created_at),
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
            ])?;
        let update_inbox = self
            .database
            .prepare(
                "UPDATE inbox
                 SET current_bundle_id = ?, revision = revision + 1, updated_at = ?
                 WHERE singleton = 1
                 RETURNING revision",
            )
            .bind(&[
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
                number(created_at),
            ])?;
        let mark_published = self
            .database
            .prepare(
                "UPDATE bundle_lifecycle
                 SET state = 'published', updated_at = ?, cleanup_after = ?
                 WHERE lifecycle_id = ? AND state = 'uploading'
                 RETURNING lifecycle_id",
            )
            .bind(&[
                number(created_at),
                number(created_at.saturating_add(CLEANUP_RETENTION_SECONDS)),
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
            ])?;

        let results = self
            .database
            .batch(vec![insert_bundle, update_inbox, mark_published])
            .await?;
        if results.len() != 3 {
            return Err(Error::RustError("publication result is incomplete".into()));
        }
        for (index, result) in results.iter().enumerate() {
            if !result.success() {
                return Err(Error::RustError(format!(
                    "publication statement {index} failed: {}",
                    result.error().unwrap_or_else(|| "unknown D1 error".into())
                )));
            }
        }
        let result = &results[1];
        let row = result
            .results::<RevisionRow>()?
            .into_iter()
            .next()
            .ok_or_else(|| Error::RustError("publication revision is missing".into()))?;
        if results[2].results::<LifecycleRow>()?.is_empty() {
            return Err(Error::RustError(
                "publication lifecycle transition did not update a row".into(),
            ));
        }
        revision(row.revision)
    }

    /// Removes expired lifecycle rows and untracked legacy objects in bounded
    /// batches. A lifecycle claim is committed before the R2 delete, so a
    /// publication that races cleanup must fail its conditional D1 batch.
    pub(crate) async fn cleanup(&self, now: u64) -> Result<CleanupReport> {
        let mut report = CleanupReport::default();
        let candidates = self.expired_lifecycle_candidates(now).await?;
        report.lifecycle_candidates = candidates.len() as u32;

        for candidate in candidates {
            if !self.claim_lifecycle(&candidate, now).await? {
                continue;
            }
            report.lifecycle_claimed += 1;

            if let Err(error) = self.bucket.delete(candidate.object_key.clone()).await {
                worker::console_error!(
                    "bundle cleanup could not delete {}: {}",
                    candidate.object_key,
                    error
                );
                report.failures += 1;
                continue;
            }

            self.finish_lifecycle_cleanup(&candidate.lifecycle_id)
                .await?;
            report.lifecycle_deleted += 1;
        }

        self.cleanup_untracked_objects(now, &mut report).await?;
        Ok(report)
    }

    async fn expired_lifecycle_candidates(&self, now: u64) -> Result<Vec<CleanupCandidate>> {
        self.database
            .prepare(
                "SELECT lifecycle_id, object_key
                 FROM bundle_lifecycle
                 WHERE cleanup_after <= ?
                   AND state IN ('uploading', 'published', 'cleanup_claimed')
                 ORDER BY cleanup_after, lifecycle_id
                 LIMIT ?",
            )
            .bind(&[number(now), number(CLEANUP_BATCH_SIZE.into())])?
            .all()
            .await?
            .results()
    }

    async fn claim_lifecycle(&self, candidate: &CleanupCandidate, now: u64) -> Result<bool> {
        let row: Option<LifecycleRow> = self
            .database
            .prepare(
                "UPDATE bundle_lifecycle
                 SET state = 'cleanup_claimed', updated_at = ?
                 WHERE lifecycle_id = ?
                   AND cleanup_after <= ?
                   AND state IN ('uploading', 'published', 'cleanup_claimed')
                   AND NOT EXISTS (
                       SELECT 1
                       FROM inbox
                       JOIN bundles ON bundles.bundle_id = inbox.current_bundle_id
                       WHERE bundles.lifecycle_id = bundle_lifecycle.lifecycle_id
                   )
                 RETURNING lifecycle_id",
            )
            .bind(&[
                number(now),
                worker::wasm_bindgen::JsValue::from_str(&candidate.lifecycle_id),
                number(now),
            ])?
            .first(None)
            .await?;
        Ok(row.is_some())
    }

    async fn finish_lifecycle_cleanup(&self, lifecycle_id: &str) -> Result<()> {
        let delete_bundle = self
            .database
            .prepare(
                "DELETE FROM bundles
                 WHERE lifecycle_id = ?
                   AND NOT EXISTS (
                       SELECT 1
                       FROM inbox
                       WHERE inbox.current_bundle_id = bundles.bundle_id
                   )",
            )
            .bind(&[worker::wasm_bindgen::JsValue::from_str(lifecycle_id)])?;
        let delete_lifecycle = self
            .database
            .prepare(
                "DELETE FROM bundle_lifecycle
                 WHERE lifecycle_id = ?
                   AND state = 'cleanup_claimed'
                   AND NOT EXISTS (
                       SELECT 1 FROM bundles
                       WHERE bundles.lifecycle_id = bundle_lifecycle.lifecycle_id
                   )",
            )
            .bind(&[worker::wasm_bindgen::JsValue::from_str(lifecycle_id)])?;
        let results = self
            .database
            .batch(vec![delete_bundle, delete_lifecycle])
            .await?;
        if results.len() != 2 {
            return Err(Error::RustError(
                "bundle cleanup result is incomplete".into(),
            ));
        }
        for (index, result) in results.iter().enumerate() {
            if !result.success() {
                return Err(Error::RustError(format!(
                    "bundle cleanup statement {index} failed: {}",
                    result.error().unwrap_or_else(|| "unknown D1 error".into())
                )));
            }
        }
        Ok(())
    }

    async fn cleanup_untracked_objects(&self, now: u64, report: &mut CleanupReport) -> Result<()> {
        let mut cursor = None;
        while report.r2_scanned < CLEANUP_MAX_R2_OBJECTS {
            let remaining = CLEANUP_MAX_R2_OBJECTS - report.r2_scanned;
            let mut request = self
                .bucket
                .list()
                .prefix(BUNDLE_OBJECT_PREFIX)
                .limit(remaining.min(CLEANUP_BATCH_SIZE));
            if let Some(cursor_value) = cursor.take() {
                request = request.cursor(cursor_value);
            }
            let page = request.execute().await?;
            let truncated = page.truncated();

            for object in page.objects() {
                report.r2_scanned += 1;
                let object_key = object.key();
                if (object.uploaded().as_millis() / 1_000).saturating_add(CLEANUP_RETENTION_SECONDS)
                    > now
                {
                    continue;
                }
                if self.object_is_tracked(&object_key).await?
                    || self.object_is_current(&object_key).await?
                {
                    continue;
                }
                match self.bucket.delete(object_key).await {
                    Ok(()) => report.r2_deleted += 1,
                    Err(error) => {
                        worker::console_error!(
                            "bundle cleanup could not delete legacy object: {}",
                            error
                        );
                        report.failures += 1;
                    }
                }
            }

            if !truncated || report.r2_scanned >= CLEANUP_MAX_R2_OBJECTS {
                report.r2_truncated = truncated && report.r2_scanned >= CLEANUP_MAX_R2_OBJECTS;
                break;
            }
            cursor = page.cursor();
            if cursor.is_none() {
                break;
            }
        }
        Ok(())
    }

    async fn object_is_tracked(&self, object_key: &str) -> Result<bool> {
        let row: Option<LifecycleRow> = self
            .database
            .prepare(
                "SELECT lifecycle_id
                 FROM bundle_lifecycle
                 WHERE object_key = ?
                 LIMIT 1",
            )
            .bind(&[worker::wasm_bindgen::JsValue::from_str(object_key)])?
            .first(None)
            .await?;
        Ok(row.is_some())
    }

    async fn object_is_current(&self, object_key: &str) -> Result<bool> {
        let row: Option<LifecycleRow> = self
            .database
            .prepare(
                "SELECT bundles.bundle_id AS lifecycle_id
                 FROM bundles
                 JOIN inbox ON inbox.current_bundle_id = bundles.bundle_id
                 WHERE bundles.object_key = ?
                 LIMIT 1",
            )
            .bind(&[worker::wasm_bindgen::JsValue::from_str(object_key)])?
            .first(None)
            .await?;
        Ok(row.is_some())
    }

    async fn reserve_lifecycle(
        &self,
        lifecycle_id: &str,
        object_key: &str,
        created_at: u64,
    ) -> Result<()> {
        let cleanup_after = created_at.saturating_add(CLEANUP_RETENTION_SECONDS);
        let result = self
            .database
            .prepare(
                "INSERT INTO bundle_lifecycle
                    (lifecycle_id, object_key, state, created_at, updated_at, cleanup_after)
                 VALUES (?, ?, 'uploading', ?, ?, ?)",
            )
            .bind(&[
                worker::wasm_bindgen::JsValue::from_str(lifecycle_id),
                worker::wasm_bindgen::JsValue::from_str(object_key),
                number(created_at),
                number(created_at),
                number(cleanup_after),
            ])?
            .run()
            .await?;
        if result.success() {
            Ok(())
        } else {
            Err(Error::RustError(result.error().unwrap_or_else(|| {
                "bundle lifecycle reservation failed".into()
            })))
        }
    }
}

/// Metadata returned after a successful publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedBundle {
    pub(crate) bundle_id: String,
    pub(crate) object_key: String,
    pub(crate) revision: InboxRevision,
    pub(crate) etag: String,
    pub(crate) manifest: Manifest,
    pub(crate) size_bytes: u64,
}

/// Current inbox metadata and the private key for its R2 object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CurrentBundle {
    pub(crate) revision: InboxRevision,
    pub(crate) etag: Option<EntityTag>,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) object_key: Option<String>,
    pub(crate) size_bytes: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CleanupReport {
    pub(crate) lifecycle_candidates: u32,
    pub(crate) lifecycle_claimed: u32,
    pub(crate) lifecycle_deleted: u32,
    pub(crate) r2_scanned: u32,
    pub(crate) r2_deleted: u32,
    pub(crate) r2_truncated: bool,
    pub(crate) failures: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredBundle {
    object_key: String,
}

#[derive(Debug, Deserialize)]
struct InboxRow {
    current_bundle_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CurrentInboxRow {
    revision: i32,
    current_bundle_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BundleMetadataRow {
    object_key: String,
    etag: String,
    manifest_json: String,
    size_bytes: i32,
}

#[derive(Debug, Deserialize)]
struct BundleRow {
    object_key: String,
}

#[derive(Debug, Deserialize)]
struct RevisionRow {
    revision: i32,
}

#[derive(Debug, Deserialize)]
struct LifecycleRow {
    lifecycle_id: String,
}

#[derive(Debug, Deserialize)]
struct CleanupCandidate {
    lifecycle_id: String,
    object_key: String,
}

fn number(value: u64) -> worker::wasm_bindgen::JsValue {
    worker::wasm_bindgen::JsValue::from_f64(value as f64)
}

fn revision(value: i32) -> Result<InboxRevision> {
    Ok(InboxRevision::new(u64::try_from(value).map_err(|_| {
        Error::RustError("inbox revision is negative".into())
    })?))
}

fn candidate_id() -> Result<String> {
    let global = worker::js_sys::global();
    let crypto =
        worker::js_sys::Reflect::get(&global, &worker::wasm_bindgen::JsValue::from_str("crypto"))?;
    let random_uuid = worker::js_sys::Reflect::get(
        &crypto,
        &worker::wasm_bindgen::JsValue::from_str("randomUUID"),
    )?;
    let random_uuid: worker::js_sys::Function = random_uuid.dyn_into()?;
    let value = random_uuid.call0(&crypto)?;
    value
        .as_string()
        .ok_or_else(|| Error::RustError("crypto.randomUUID returned a non-string".into()))
}

fn candidate_object_key(bundle_id: &str) -> String {
    format!("{CANDIDATE_OBJECT_PREFIX}{bundle_id}.tar")
}

fn validate_for_publication(bytes: &[u8]) -> Result<Manifest> {
    if bytes.len() as u64 > MAX_BUNDLE_SIZE {
        return Err(invalid_bundle("bundle exceeds the protocol size limit"));
    }
    validate(Cursor::new(bytes))
        .map(|bundle| bundle.into_manifest())
        .map_err(|error| invalid_bundle(format!("bundle validation failed: {error}")))
}

fn invalid_bundle(message: impl Into<String>) -> Error {
    Error::RustError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_objects_have_a_cleanup_prefix() {
        assert_eq!(
            candidate_object_key("bundle-1"),
            "bundles/candidates/bundle-1.tar"
        );
        assert!(candidate_object_key("bundle-1").starts_with(CANDIDATE_OBJECT_PREFIX));
    }

    #[test]
    fn invalid_bundle_bytes_are_rejected_before_publication() {
        assert!(validate_for_publication(b"not a PRSync archive").is_err());
    }

    #[test]
    fn publication_metadata_keeps_the_r2_identity() {
        let published = PublishedBundle {
            bundle_id: "bundle-1".into(),
            object_key: candidate_object_key("bundle-1"),
            revision: InboxRevision::new(3),
            etag: "etag-1".into(),
            manifest: Manifest {
                protocol_version: prs_sync_protocol::CURRENT_PROTOCOL_VERSION,
                bundle_format_version: prs_sync_protocol::CURRENT_BUNDLE_FORMAT_VERSION,
                entry_point: prs_sync_protocol::BundlePath::new("index.md").unwrap(),
                files: Vec::new(),
            },
            size_bytes: 42,
        };

        assert_eq!(
            published.object_key,
            candidate_object_key(&published.bundle_id)
        );
        assert!(!published.etag.is_empty());
        assert_eq!(published.size_bytes, 42);
    }
}
