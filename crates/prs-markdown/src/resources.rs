//! Device-independent document and asset loading.
//!
//! [`ResourceProvider`] is the boundary between the reader and the storage
//! medium.  The reader deals in paths relative to a configured document root;
//! a provider decides how those paths are read.  The filesystem implementation
//! below is useful on a host and on the T1, while another implementation can
//! retrieve the same paths from a package, archive, or device service.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Read};
use std::path::Component;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ResourceId(String);

impl ResourceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl AsRef<str> for ResourceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKind {
    Image,
    Stylesheet,
    Include,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceRequest {
    pub id: ResourceId,
    pub kind: ResourceKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    pub media_type: Option<String>,
    pub bytes: Vec<u8>,
}

/// The result of resolving a Markdown reference without deciding what action
/// a reader should take for it.
///
/// Local paths are normalized and relative to the provider's configured root.
/// An [`Anchor`](Self::Anchor) has no path because it refers to the containing
/// document.  External references retain their original URL and are never
/// passed to the filesystem provider.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceTarget {
    Anchor(String),
    Document(PathBuf),
    DocumentAnchor { document: PathBuf, anchor: String },
    Asset(PathBuf),
    External(String),
}

impl ResourceTarget {
    pub fn document_path(&self) -> Option<&Path> {
        match self {
            Self::Document(path) | Self::Asset(path) => Some(path),
            Self::DocumentAnchor { document, .. } => Some(document),
            Self::Anchor(_) | Self::External(_) => None,
        }
    }
}

/// Storage abstraction consumed by the Markdown reader.
///
/// Every local path passed to this interface is in the provider's path
/// namespace.  Implementations may use any backing store, but must resolve a
/// reference from the containing document supplied by the caller rather than
/// from the process working directory.
pub trait ResourceProvider {
    /// The entry-point document in the provider's logical path namespace.
    fn document_path(&self) -> &Path;

    /// Return the document opened by [`crate::reader::Reader::open`].
    ///
    /// This is a logical resource identifier, not a process filesystem path.
    /// Browser-backed providers can use the same reader with an in-memory or
    /// URL-backed namespace.
    fn entry_point(&self) -> &Path {
        self.document_path()
    }

    fn current_document_path(&self) -> &Path {
        self.document_path()
    }

    /// Read a UTF-8 Markdown or other text resource.
    fn read_text(&self, path: &Path) -> Result<String, ResourceError>;

    /// Read an opaque binary resource such as an image.
    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError>;

    /// Read a binary resource with an encoded-byte bound.
    ///
    /// Providers with a streaming or filesystem backend should override this
    /// method so the bound applies before allocation. The default keeps the
    /// boundary useful for simple providers, although such providers may
    /// still allocate their full source before this check.
    fn read_binary_limited(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, ResourceError> {
        let bytes = self.read_binary(path)?;
        if bytes.len() > max_bytes {
            return Err(ResourceError::new(format!(
                "binary resource exceeds {} byte limit: {}",
                max_bytes,
                path.display()
            )));
        }
        Ok(bytes)
    }

    /// Resolve a reference from the current document.
    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError>;

    fn resolve(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference(reference)
    }

    /// Resolve a reference from an explicitly supplied containing document.
    fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError>;

    fn resolve_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(containing_document, reference)
    }

    /// Compatibility convenience for callers that need the original generic
    /// resource record.  Text-like resources are encoded as UTF-8 bytes.
    fn load(&self, request: &ResourceRequest) -> Result<Resource, ResourceError> {
        let path = Path::new(request.id.as_ref());
        let bytes = match request.kind {
            ResourceKind::Image => self.read_binary(path)?,
            ResourceKind::Stylesheet | ResourceKind::Include => self.read_text(path)?.into_bytes(),
        };

        Ok(Resource {
            id: request.id.clone(),
            kind: request.kind,
            media_type: None,
            bytes,
        })
    }

    fn read_markdown(&self, path: &Path) -> Result<String, ResourceError> {
        self.read_text(path)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceError {
    message: String,
}

impl ResourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ResourceError {}

/// A root-relative, in-memory [`ResourceProvider`] for browser libraries.
///
/// The browser File System Access API is asynchronous, while the reader's
/// resource boundary is deliberately synchronous. The browser adapter reads
/// the selected directory into this provider before constructing a reader.
/// Paths remain in the same logical namespace as the native filesystem
/// provider, and every path is normalized before it can be stored or read.
#[derive(Clone, Debug)]
pub struct BrowserResourceProvider {
    files: HashMap<PathBuf, Vec<u8>>,
    document: PathBuf,
}

impl BrowserResourceProvider {
    /// Create a provider from files whose paths are relative to the selected
    /// directory.
    pub fn new<I, P>(entry_point: P, files: I) -> Result<Self, ResourceError>
    where
        I: IntoIterator<Item = (PathBuf, Vec<u8>)>,
        P: AsRef<Path>,
    {
        let document = normalize_relative(entry_point.as_ref())?;
        if document.as_os_str().is_empty() {
            return Err(ResourceError::new("browser entry point cannot be empty"));
        }

        let mut normalized_files = HashMap::new();
        for (path, bytes) in files {
            let path = normalize_relative(&path)?;
            if path.as_os_str().is_empty() {
                return Err(ResourceError::new("browser resource path cannot be empty"));
            }
            normalized_files.insert(path, bytes);
        }

        if !normalized_files.contains_key(&document) {
            return Err(ResourceError::new(format!(
                "browser entry point is missing: {}",
                document.display()
            )));
        }

        Ok(Self {
            files: normalized_files,
            document,
        })
    }

    pub fn document_path(&self) -> &Path {
        &self.document
    }

    pub fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        let path = self.namespace_path(path)?;
        let bytes = self.file(&path)?;
        String::from_utf8(bytes.to_vec()).map_err(|error| {
            ResourceError::new(format!(
                "read text resource '{}': invalid UTF-8 ({error})",
                path.display()
            ))
        })
    }

    pub fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        self.read_binary_limited(path, usize::MAX)
    }

    pub fn read_binary_limited(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ResourceError> {
        let path = self.namespace_path(path)?;
        let bytes = self.file(&path)?;
        if bytes.len() > max_bytes {
            return Err(ResourceError::new(format!(
                "binary resource exceeds {} byte limit: {}",
                max_bytes,
                path.display()
            )));
        }
        Ok(bytes.to_vec())
    }

    pub fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(&self.document, reference)
    }

    pub fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        if is_external_reference(reference) {
            return Ok(ResourceTarget::External(reference.to_owned()));
        }
        let containing_document = self.namespace_path(containing_document)?;
        resolve_reference_in_namespace(&containing_document, reference)
    }

    fn namespace_path(&self, path: &Path) -> Result<PathBuf, ResourceError> {
        normalize_relative(path)
    }

    fn file(&self, path: &Path) -> Result<&[u8], ResourceError> {
        self.files.get(path).map(Vec::as_slice).ok_or_else(|| {
            ResourceError::new(format!("browser resource is missing: {}", path.display()))
        })
    }
}

impl ResourceProvider for BrowserResourceProvider {
    fn document_path(&self) -> &Path {
        self.document_path()
    }

    fn entry_point(&self) -> &Path {
        self.document_path()
    }

    fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        self.read_text(path)
    }

    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        self.read_binary(path)
    }

    fn read_binary_limited(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, ResourceError> {
        self.read_binary_limited(path, max_bytes)
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference(reference)
    }

    fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(containing_document, reference)
    }
}

/// A filesystem-backed [`ResourceProvider`].
///
/// `root` is canonicalized when the provider is created.  All normalized
/// local paths must remain below that root; `..` references that would escape
/// it are rejected.  Existing symlinks are checked too, so a path cannot use a
/// symlink inside the root to reach outside it.  The current document and all
/// resolved local targets are exposed as root-relative paths.
#[derive(Clone, Debug)]
#[cfg(not(target_arch = "wasm32"))]
pub struct FileSystemResourceProvider {
    root: PathBuf,
    document: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl FileSystemResourceProvider {
    pub fn new(root: impl AsRef<Path>, document: impl AsRef<Path>) -> Result<Self, ResourceError> {
        let root = fs::canonicalize(root.as_ref()).map_err(|error| {
            ResourceError::io("canonicalize resource root", root.as_ref(), error)
        })?;
        let document = namespace_path(&root, document.as_ref())?;

        Ok(Self { root, document })
    }

    pub fn from_root(root: impl AsRef<Path>) -> Result<Self, ResourceError> {
        Self::new(root, Path::new(""))
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn document_path(&self) -> &Path {
        &self.document
    }

    pub fn current_document_path(&self) -> &Path {
        self.document_path()
    }

    pub fn read_markdown(&self, path: &Path) -> Result<String, ResourceError> {
        self.read_text(path)
    }

    pub fn with_document(&self, document: impl AsRef<Path>) -> Result<Self, ResourceError> {
        let document = namespace_path(&self.root, document.as_ref())?;
        Ok(Self {
            root: self.root.clone(),
            document,
        })
    }

    pub fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        let path = self.filesystem_path(path)?;
        fs::read_to_string(&path)
            .map_err(|error| ResourceError::io("read text resource", &path, error))
    }

    pub fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        self.read_binary_limited(path, usize::MAX)
    }

    pub fn read_binary_limited(
        &self,
        path: &Path,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ResourceError> {
        let path = self.filesystem_path(path)?;
        let metadata = fs::metadata(&path)
            .map_err(|error| ResourceError::io("inspect binary resource", &path, error))?;
        if metadata.len() > max_bytes as u64 {
            return Err(ResourceError::new(format!(
                "binary resource exceeds {} byte limit: {}",
                max_bytes,
                path.display()
            )));
        }
        let capacity = usize::try_from(metadata.len()).unwrap_or(max_bytes);
        let mut file = fs::File::open(&path)
            .map_err(|error| ResourceError::io("open binary resource", &path, error))?;
        let mut bytes = Vec::with_capacity(capacity.min(max_bytes));
        file.read_to_end(&mut bytes)
            .map_err(|error| ResourceError::io("read binary resource", &path, error))?;
        Ok(bytes)
    }

    pub fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(&self.document, reference)
    }

    pub fn resolve(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference(reference)
    }

    pub fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        if is_external_reference(reference) {
            return Ok(ResourceTarget::External(reference.to_owned()));
        }
        let containing_document = namespace_path(&self.root, containing_document)?;
        resolve_reference_in_namespace(&containing_document, reference)
    }

    pub fn resolve_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(containing_document, reference)
    }

    fn filesystem_path(&self, path: &Path) -> Result<PathBuf, ResourceError> {
        let relative = namespace_path(&self.root, path)?;
        let filesystem_path = self.root.join(relative);
        self.ensure_within_root(&filesystem_path)?;
        Ok(filesystem_path)
    }

    fn ensure_within_root(&self, path: &Path) -> Result<(), ResourceError> {
        let mut existing = path.to_path_buf();
        let canonical = loop {
            match fs::canonicalize(&existing) {
                Ok(canonical) => break canonical,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if !existing.pop() {
                        return Err(ResourceError::io("resolve resource path", path, error));
                    }
                }
                Err(error) => {
                    return Err(ResourceError::io("resolve resource path", path, error));
                }
            }
        };

        if canonical.starts_with(&self.root) {
            Ok(())
        } else {
            Err(ResourceError::new(format!(
                "resource path escapes root: {}",
                path.display()
            )))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl ResourceProvider for FileSystemResourceProvider {
    fn document_path(&self) -> &Path {
        self.document_path()
    }

    fn entry_point(&self) -> &Path {
        self.document_path()
    }

    fn read_text(&self, path: &Path) -> Result<String, ResourceError> {
        self.read_text(path)
    }

    fn read_binary(&self, path: &Path) -> Result<Vec<u8>, ResourceError> {
        self.read_binary(path)
    }

    fn read_binary_limited(&self, path: &Path, max_bytes: usize) -> Result<Vec<u8>, ResourceError> {
        self.read_binary_limited(path, max_bytes)
    }

    fn resolve_reference(&self, reference: &str) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference(reference)
    }

    fn resolve_reference_from(
        &self,
        containing_document: &Path,
        reference: &str,
    ) -> Result<ResourceTarget, ResourceError> {
        self.resolve_reference_from(containing_document, reference)
    }
}

/// British-spelling alias for callers that use “filesystem” consistently.
#[cfg(not(target_arch = "wasm32"))]
pub type FilesystemResourceProvider = FileSystemResourceProvider;
#[cfg(not(target_arch = "wasm32"))]
pub type FileSystemResources = FileSystemResourceProvider;
#[cfg(not(target_arch = "wasm32"))]
pub type FilesystemResources = FileSystemResourceProvider;

#[cfg(not(target_arch = "wasm32"))]
fn namespace_path(root: &Path, path: &Path) -> Result<PathBuf, ResourceError> {
    if path.is_absolute() {
        let relative = path.strip_prefix(root).map_err(|_| {
            ResourceError::new(format!("path is outside resource root: {}", path.display()))
        })?;
        normalize_relative(relative)
    } else {
        normalize_relative(path)
    }
}

fn normalize_relative(path: &Path) -> Result<PathBuf, ResourceError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ResourceError::new(format!(
                        "path escapes resource root: {}",
                        path.display()
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ResourceError::new(format!(
                    "absolute path is not in the resource namespace: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(normalized)
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

fn is_external_reference(reference: &str) -> bool {
    if reference.starts_with("//") {
        return true;
    }

    let Some(colon) = reference.find(':') else {
        return false;
    };
    let scheme = &reference[..colon];
    !scheme.is_empty()
        && scheme.chars().enumerate().all(|(index, character)| {
            if index == 0 {
                character.is_ascii_alphabetic()
            } else {
                character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
            }
        })
}

fn resolve_reference_in_namespace(
    containing_document: &Path,
    reference: &str,
) -> Result<ResourceTarget, ResourceError> {
    if is_external_reference(reference) {
        return Ok(ResourceTarget::External(reference.to_owned()));
    }

    let (path_reference, anchor) = reference
        .split_once('#')
        .map(|(path, fragment)| (path, Some(fragment.to_owned())))
        .unwrap_or((reference, None));

    if path_reference.is_empty() {
        return match anchor {
            Some(anchor) => Ok(ResourceTarget::Anchor(anchor)),
            None => Ok(ResourceTarget::Document(containing_document.to_owned())),
        };
    }

    if Path::new(path_reference).is_absolute() {
        return Err(ResourceError::new(format!(
            "absolute local reference is not allowed: {path_reference}"
        )));
    }

    let parent = containing_document
        .parent()
        .unwrap_or_else(|| Path::new(""));
    let document = normalize_relative(&parent.join(path_reference))?;
    if is_markdown_path(&document) {
        return match anchor {
            Some(anchor) => Ok(ResourceTarget::DocumentAnchor { document, anchor }),
            None => Ok(ResourceTarget::Document(document)),
        };
    }

    if anchor.is_some() {
        return Err(ResourceError::new(format!(
            "anchors are only supported on Markdown documents: {path_reference}"
        )));
    }
    Ok(ResourceTarget::Asset(document))
}

#[cfg(not(target_arch = "wasm32"))]
impl ResourceError {
    fn io(operation: &str, path: &Path, error: io::Error) -> Self {
        Self::new(format!("{operation} '{}': {error}", path.display()))
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "prs-markdown-resources-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create temporary resource root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn filesystem_provider_reads_text_and_binary_resources() {
        let root = TestRoot::new();
        fs::create_dir_all(root.path().join("guide/part one/images"))
            .expect("create nested resource directories");
        fs::write(root.path().join("guide/part one/chapter.md"), "# Chapter\n")
            .expect("write Markdown document");
        fs::write(
            root.path().join("guide/part one/images/chart final.png"),
            [0_u8, 1, 2, 255],
        )
        .expect("write image resource");

        let provider =
            FileSystemResourceProvider::new(root.path(), Path::new("guide/part one/chapter.md"))
                .expect("create provider");

        assert_eq!(
            provider.document_path(),
            Path::new("guide/part one/chapter.md")
        );
        assert_eq!(
            provider.read_text(Path::new("./guide/part one/chapter.md")),
            Ok("# Chapter\n".to_owned())
        );
        assert_eq!(
            provider.read_binary(Path::new("guide/part one/images/chart final.png")),
            Ok(vec![0, 1, 2, 255])
        );
        assert!(provider
            .read_binary_limited(Path::new("guide/part one/images/chart final.png"), 3)
            .is_err());
        assert_eq!(
            provider.read_binary_limited(Path::new("guide/part one/images/chart final.png"), 4),
            Ok(vec![0, 1, 2, 255])
        );
    }

    #[test]
    fn references_resolve_from_containing_document_directory() {
        let root = TestRoot::new();
        let provider = FileSystemResourceProvider::new(root.path(), "guide/part one/chapter.md")
            .expect("create provider");

        assert_eq!(
            provider.resolve_reference("sibling.md"),
            Ok(ResourceTarget::Document(PathBuf::from(
                "guide/part one/sibling.md"
            )))
        );
        assert_eq!(
            provider.resolve_reference("../notes/foo.md"),
            Ok(ResourceTarget::Document(PathBuf::from(
                "guide/notes/foo.md"
            )))
        );
        assert_eq!(
            provider.resolve_reference("../notes/foo.md#results"),
            Ok(ResourceTarget::DocumentAnchor {
                document: PathBuf::from("guide/notes/foo.md"),
                anchor: "results".into(),
            })
        );
        assert_eq!(
            provider.resolve_reference_from(Path::new("guide/part two/index.md"), "../shared.md"),
            Ok(ResourceTarget::Document(PathBuf::from("guide/shared.md")))
        );
        assert_eq!(
            provider.resolve_reference("./images/chart final.png"),
            Ok(ResourceTarget::Asset(PathBuf::from(
                "guide/part one/images/chart final.png"
            )))
        );
    }

    #[test]
    fn fragment_and_document_anchor_targets_are_distinct() {
        let root = TestRoot::new();
        let provider =
            FileSystemResourceProvider::new(root.path(), "docs/index.md").expect("create provider");

        assert_eq!(
            provider.resolve_reference("#installation"),
            Ok(ResourceTarget::Anchor("installation".into()))
        );
        assert_eq!(
            provider.resolve_reference("file.md#results"),
            Ok(ResourceTarget::DocumentAnchor {
                document: PathBuf::from("docs/file.md"),
                anchor: "results".into(),
            })
        );
        assert_eq!(
            provider.resolve_reference("file.MARKDOWN"),
            Ok(ResourceTarget::Document(PathBuf::from(
                "docs/file.MARKDOWN"
            )))
        );
    }

    #[test]
    fn external_urls_are_not_treated_as_filesystem_paths() {
        let root = TestRoot::new();
        let provider =
            FileSystemResourceProvider::new(root.path(), "index.md").expect("create provider");

        for url in [
            "https://example.com/docs#results",
            "mailto:reader@example.com",
            "//cdn.example.com/chart.png",
        ] {
            assert_eq!(
                provider.resolve_reference(url),
                Ok(ResourceTarget::External(url.into()))
            );
        }
    }

    #[test]
    fn missing_resources_return_errors_without_changing_resolution() {
        let root = TestRoot::new();
        let provider =
            FileSystemResourceProvider::new(root.path(), "index.md").expect("create provider");

        assert!(provider.read_text(Path::new("missing.md")).is_err());
        assert!(provider.read_binary(Path::new("missing.png")).is_err());
        assert_eq!(
            provider.resolve_reference("missing.md"),
            Ok(ResourceTarget::Document(PathBuf::from("missing.md")))
        );
    }

    #[test]
    fn references_cannot_escape_configured_root() {
        let root = TestRoot::new();
        let provider =
            FileSystemResourceProvider::new(root.path(), "docs/index.md").expect("create provider");

        assert!(provider.resolve_reference("../../outside.md").is_err());
        assert!(provider
            .resolve_reference_from(Path::new("../outside.md"), "next.md")
            .is_err());
        assert!(provider.read_text(Path::new("../outside.md")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reads_reject_symlinks_that_leave_configured_root() {
        let root = TestRoot::new();
        let outside = TestRoot::new();
        fs::write(outside.path().join("secret.png"), [7_u8, 8, 9]).expect("write outside asset");
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked"))
            .expect("create outside symlink");

        let provider =
            FileSystemResourceProvider::new(root.path(), "index.md").expect("create provider");
        assert!(provider
            .read_binary(Path::new("linked/secret.png"))
            .is_err());
    }

    #[test]
    fn browser_provider_reads_nested_documents_and_assets() {
        let provider = BrowserResourceProvider::new(
            "README.md",
            [
                (
                    PathBuf::from("README.md"),
                    b"# Home\n\n[Chapter](docs/chapter.md)\n".to_vec(),
                ),
                (PathBuf::from("docs/chapter.md"), b"# Chapter\n".to_vec()),
                (PathBuf::from("assets/cover.png"), vec![1, 2, 3]),
            ],
        )
        .expect("create browser provider");

        assert_eq!(provider.document_path(), Path::new("README.md"));
        assert_eq!(
            provider.read_text(Path::new("./README.md")),
            Ok("# Home\n\n[Chapter](docs/chapter.md)\n".to_owned())
        );
        assert_eq!(
            provider.read_binary(Path::new("assets/cover.png")),
            Ok(vec![1, 2, 3])
        );
        assert_eq!(
            provider.resolve_reference("docs/chapter.md"),
            Ok(ResourceTarget::Document(PathBuf::from("docs/chapter.md")))
        );
        assert_eq!(
            provider.resolve_reference("assets/cover.png"),
            Ok(ResourceTarget::Asset(PathBuf::from("assets/cover.png")))
        );
    }

    #[test]
    fn browser_provider_rejects_missing_entry_and_path_escape() {
        assert!(BrowserResourceProvider::new(
            "README.md",
            [(PathBuf::from("docs/README.md"), b"wrong root\n".to_vec())],
        )
        .is_err());

        let provider = BrowserResourceProvider::new(
            "docs/README.md",
            [(PathBuf::from("docs/README.md"), b"# Docs\n".to_vec())],
        )
        .expect("create browser provider");
        assert!(provider.read_text(Path::new("../../outside.md")).is_err());
        assert!(provider.resolve_reference("../../../outside.md").is_err());
        assert!(provider
            .resolve_reference_from(Path::new("../outside.md"), "next.md")
            .is_err());
        assert!(provider
            .read_binary_limited(Path::new("docs/README.md"), 2)
            .is_err());
    }
}
