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

/// Candidate objects are private Worker storage. Cleanup can list this prefix
/// and delete every object except the object referenced by the inbox.
pub(crate) const CANDIDATE_OBJECT_PREFIX: &str = "bundles/candidates/";

const CANDIDATE_STATE_METADATA: &str = "candidate";

/// The Worker-owned D1 and R2 handles for bundle publication.
#[derive(Debug)]
pub(crate) struct BundleStore {
    database: D1Database,
    bucket: Bucket,
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
    pub(crate) async fn push(&self, bytes: Vec<u8>, updated_at: u64) -> Result<PublishedBundle> {
        let previous = self.current_bundle().await?;
        self.clear_inbox(updated_at).await?;
        if let Some(previous) = previous {
            self.bucket.delete(previous.object_key).await?;
        }

        let manifest = validate_for_publication(&bytes)?;

        let bundle_id = candidate_id()?;
        let object_key = candidate_object_key(&bundle_id);
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
            Error::RustError(format!("failed to encode bundle manifest: {error}"))
        })?;

        self.publish(
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
    pub(crate) async fn clear(&self, updated_at: u64) -> Result<()> {
        let previous = self.current_bundle().await?;
        self.clear_inbox(updated_at).await?;
        if let Some(previous) = previous {
            self.bucket.delete(previous.object_key).await?;
        }
        Ok(())
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

    async fn clear_inbox(&self, updated_at: u64) -> Result<()> {
        self.database
            .prepare(
                "UPDATE inbox SET current_bundle_id = NULL, revision = revision + 1, updated_at = ? WHERE singleton = 1",
            )
            .bind(&[number(updated_at)])?
            .run()
            .await?;
        Ok(())
    }

    async fn publish(
        &self,
        bundle_id: &str,
        object_key: &str,
        etag: &str,
        manifest_json: &str,
        size_bytes: u64,
        created_at: u64,
    ) -> Result<()> {
        let insert_bundle = self
            .database
            .prepare(
                "INSERT INTO bundles (bundle_id, object_key, etag, manifest_json, size_bytes, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&[
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
                worker::wasm_bindgen::JsValue::from_str(object_key),
                worker::wasm_bindgen::JsValue::from_str(etag),
                worker::wasm_bindgen::JsValue::from_str(manifest_json),
                number(size_bytes),
                number(created_at),
            ])?;
        let update_inbox = self
            .database
            .prepare(
                "UPDATE inbox SET current_bundle_id = ?, revision = revision + 1, updated_at = ? WHERE singleton = 1",
            )
            .bind(&[
                worker::wasm_bindgen::JsValue::from_str(bundle_id),
                number(created_at),
            ])?;

        self.database
            .batch(vec![insert_bundle, update_inbox])
            .await?;
        Ok(())
    }
}

/// Metadata returned after a successful publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PublishedBundle {
    pub(crate) bundle_id: String,
    pub(crate) object_key: String,
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
