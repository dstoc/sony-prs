//! Safe generation, validation, and extraction of PRSync bundles.
//!
//! A bundle is an uncompressed tar archive. The first archive entry is the
//! generated `manifest.json`; the remaining entries are regular files listed
//! by that manifest. Validation and extraction consume the archive as a
//! stream. Extraction writes to a private staging directory and publishes
//! that directory only after the complete archive passes validation.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Write};
use std::path::{Component, Path, PathBuf};

use prs_sync_protocol::{
    BundlePath, Manifest, ManifestFile, ProtocolVersion, CURRENT_BUNDLE_FORMAT_VERSION,
    CURRENT_PROTOCOL_VERSION, MAX_BUNDLE_SIZE,
};
use sha2::{Digest, Sha256};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::Builder as TempDirBuilder;

/// The archive-root metadata entry.
pub const MANIFEST_PATH: &str = "manifest.json";

/// The largest encoded bundle accepted by this crate.
pub const MAX_ARCHIVE_SIZE: u64 = MAX_BUNDLE_SIZE;

/// The largest UTF-8 bundle path that fits in a regular UStar header.
pub const MAX_BUNDLE_PATH_BYTES: usize = 255;

/// The largest manifest accepted while validating a stream.
///
/// The manifest is the only bundle part held in memory. It cannot exceed the
/// encoded archive limit.
pub const MAX_MANIFEST_SIZE: u64 = MAX_ARCHIVE_SIZE;

/// A source file selected for inclusion in a generated bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleSource {
    /// The source path on the host filesystem.
    pub source: PathBuf,
    /// The path stored in the archive.
    pub bundle_path: BundlePath,
    /// The source file size in bytes.
    pub size: u64,
    /// Lowercase SHA-256 captured while collecting this source.
    pub sha256: String,
}

/// A completely validated bundle represented in memory for browser WASM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InMemoryBundle {
    pub manifest: Manifest,
    pub files: Vec<(BundlePath, Vec<u8>)>,
}

/// Builds a bundle from one Markdown entry point and explicitly selected files.
#[derive(Debug, Clone)]
pub struct BundleBuilder {
    entry_point: PathBuf,
    additional_files: Vec<PathBuf>,
}

impl BundleBuilder {
    /// Creates a builder whose first source path is the bundle entry point.
    pub fn new(entry_point: impl AsRef<Path>) -> Self {
        Self {
            entry_point: entry_point.as_ref().to_path_buf(),
            additional_files: Vec::new(),
        }
    }

    /// Adds one explicitly selected regular file.
    pub fn add_file(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.additional_files.push(path.as_ref().to_path_buf());
        self
    }

    /// Adds explicitly selected regular files.
    pub fn add_files<I, P>(&mut self, paths: I) -> &mut Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        self.additional_files
            .extend(paths.into_iter().map(|path| path.as_ref().to_path_buf()));
        self
    }

    /// Returns the entry point source path.
    pub fn entry_point(&self) -> &Path {
        &self.entry_point
    }

    /// Writes one uncompressed tar archive to `writer`.
    pub fn write<W: Write>(&self, writer: W) -> Result<Manifest, BundleError> {
        let sources = collect_sources(&self.entry_point, &self.additional_files)?;
        let manifest = make_manifest(&sources)?;
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        let mut limited = LimitedWriter::new(writer, MAX_ARCHIVE_SIZE);
        let archive_result = {
            let mut archive = Builder::new(&mut limited);
            (|| {
                append_bytes(&mut archive, MANIFEST_PATH, &manifest_bytes)?;
                for source in &sources {
                    append_source(&mut archive, source)?;
                }
                archive.finish().map_err(BundleError::from)
            })()
        };

        if limited.exceeded() {
            return Err(BundleError::ArchiveTooLarge {
                limit: MAX_ARCHIVE_SIZE,
            });
        }
        archive_result?;
        Ok(manifest)
    }
}

/// Generates a bundle from an entry point and explicit additional files.
pub fn create_bundle<W, I, P>(
    entry_point: impl AsRef<Path>,
    additional_files: I,
    writer: W,
) -> Result<Manifest, BundleError>
where
    W: Write,
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut builder = BundleBuilder::new(entry_point);
    builder.add_files(additional_files);
    builder.write(writer)
}

/// The result of validating a complete bundle stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedBundle {
    manifest: Manifest,
}

impl ValidatedBundle {
    /// Returns the validated manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Consumes the result and returns its manifest.
    pub fn into_manifest(self) -> Manifest {
        self.manifest
    }
}

/// Validates a complete uncompressed tar stream without extracting it.
pub fn validate<R: Read>(reader: R) -> Result<ValidatedBundle, BundleError> {
    process_archive(reader, None, None, None, false).map(|manifest| ValidatedBundle { manifest })
}

/// Validate and collect a bounded bundle in memory, requiring a SHA-256 for
/// every file. Returned bytes are safe to activate only after this succeeds.
pub fn extract_to_memory<R: Read>(reader: R) -> Result<InMemoryBundle, BundleError> {
    let mut files = Vec::new();
    let manifest = process_archive(reader, None, None, Some(&mut files), true)?;
    Ok(InMemoryBundle { manifest, files })
}

/// Validates and extracts a complete bundle stream.
///
/// The destination must not contain files. The archive is first extracted to
/// a private sibling directory. The destination is replaced only after the
/// complete archive has passed validation. This keeps invalid or truncated
/// input from becoming visible to readers of the destination.
pub fn extract<R: Read>(reader: R, destination: impl AsRef<Path>) -> Result<Manifest, BundleError> {
    extract_internal(reader, destination.as_ref(), None)
}

/// Validates and extracts a complete bundle stream within a caller-provided
/// staging-content limit.
///
/// The limit includes the raw `manifest.json` entry and every extracted file.
/// Extraction remains private until the complete archive passes validation and
/// the limit check.
pub fn extract_with_size_limit<R: Read>(
    reader: R,
    destination: impl AsRef<Path>,
    staging_limit: u64,
) -> Result<Manifest, BundleError> {
    extract_internal(reader, destination.as_ref(), Some(staging_limit))
}

fn extract_internal<R: Read>(
    reader: R,
    destination: &Path,
    staging_limit: Option<u64>,
) -> Result<Manifest, BundleError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent_metadata = fs::metadata(parent).map_err(BundleError::Io)?;
    if !parent_metadata.is_dir() {
        return Err(BundleError::DestinationParentNotDirectory(
            parent.to_path_buf(),
        ));
    }
    if fs::symlink_metadata(destination).is_ok() {
        let metadata = fs::symlink_metadata(destination).map_err(BundleError::Io)?;
        if metadata.file_type().is_dir() && fs::read_dir(destination)?.next().is_none() {
            // An empty caller-created directory is a supported destination.
        } else {
            return Err(BundleError::DestinationExists(destination.to_path_buf()));
        }
    }

    let staging = TempDirBuilder::new()
        .prefix(".prs-sync-bundle-")
        .tempdir_in(parent)
        .map_err(BundleError::Io)?;
    let manifest = process_archive(reader, Some(staging.path()), staging_limit, None, false)?;

    if fs::symlink_metadata(destination).is_ok() {
        fs::remove_dir(destination).map_err(BundleError::Io)?;
    }
    fs::rename(staging.path(), destination).map_err(BundleError::Io)?;
    // The directory has been moved into place. TempDir cleanup sees a missing
    // path and leaves the published destination intact.
    Ok(manifest)
}

/// A bundle generation or validation failure.
#[derive(Debug)]
pub enum BundleError {
    /// A host filesystem or stream operation failed.
    Io(io::Error),
    /// The generated or received JSON manifest is invalid.
    Json(serde_json::Error),
    /// The encoded archive exceeds the fixed protocol limit.
    ArchiveTooLarge { limit: u64 },
    /// The manifest exceeds the bounded parser limit.
    ManifestTooLarge { limit: u64 },
    /// The archive contains bytes after the tar end marker.
    TrailingArchiveData,
    /// A path is not a safe bundle-relative path.
    UnsafePath(String),
    /// The manifest uses a protocol version that this crate cannot read.
    UnsupportedProtocolVersion { received: ProtocolVersion },
    /// The manifest uses a bundle format version that this crate cannot read.
    UnsupportedBundleVersion { received: u16 },
    /// The entry point is not a Markdown path.
    EntryPointNotMarkdown(BundlePath),
    /// The manifest contains the same path more than once.
    DuplicateManifestPath(BundlePath),
    /// The manifest lists the archive metadata entry as a document file.
    ManifestListsManifest,
    /// The manifest entry point is not listed in the file list.
    EntryPointMissingFromManifest(BundlePath),
    /// The tar archive contains an entry that is not a regular file.
    UnsupportedEntry { path: String, entry_type: String },
    /// The same archive path occurs more than once.
    DuplicateArchivePath(BundlePath),
    /// A regular archive entry is not listed by the manifest.
    UnexpectedArchivePath(BundlePath),
    /// A manifest-listed file is absent from the archive.
    MissingArchivePath(BundlePath),
    /// A tar header size does not match the bytes available in the stream.
    EntrySizeMismatch {
        path: BundlePath,
        expected: u64,
        actual: u64,
    },
    /// A regular archive entry size differs from its manifest size.
    ManifestSizeMismatch {
        path: BundlePath,
        expected: u64,
        actual: u64,
    },
    /// A browser bundle omitted a required integrity hash.
    MissingFileHash(BundlePath),
    /// A manifest hash is not 64 lowercase hexadecimal characters.
    InvalidFileHash(BundlePath),
    /// A regular archive entry does not match its manifest SHA-256.
    FileHashMismatch { path: BundlePath },
    /// The sum of manifest file sizes exceeds the bounded protocol limit.
    ExtractedSizeTooLarge { limit: u64 },
    /// The raw manifest and extracted files exceed a caller-provided staging limit.
    StagingSizeTooLarge { limit: u64 },
    /// The extraction target already contains data or is not a directory.
    DestinationExists(PathBuf),
    /// The destination parent is not a directory.
    DestinationParentNotDirectory(PathBuf),
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "I/O error: {error}"),
            Self::Json(error) => write!(formatter, "invalid bundle manifest: {error}"),
            Self::ArchiveTooLarge { limit } => {
                write!(formatter, "bundle archive exceeds {limit} bytes")
            }
            Self::ManifestTooLarge { limit } => {
                write!(formatter, "bundle manifest exceeds {limit} bytes")
            }
            Self::TrailingArchiveData => write!(formatter, "bundle contains trailing archive data"),
            Self::UnsafePath(path) => write!(formatter, "unsafe bundle path `{path}`"),
            Self::UnsupportedProtocolVersion { received } => write!(
                formatter,
                "unsupported protocol version {}.{}",
                received.major, received.minor
            ),
            Self::UnsupportedBundleVersion { received } => {
                write!(formatter, "unsupported bundle format version {received}")
            }
            Self::EntryPointNotMarkdown(path) => {
                write!(formatter, "bundle entry point is not Markdown: {path:?}")
            }
            Self::DuplicateManifestPath(path) => {
                write!(formatter, "manifest lists duplicate path {path:?}")
            }
            Self::ManifestListsManifest => {
                write!(
                    formatter,
                    "manifest must not list manifest.json as a document file"
                )
            }
            Self::EntryPointMissingFromManifest(path) => write!(
                formatter,
                "manifest entry point is not listed as a document file: {path:?}"
            ),
            Self::UnsupportedEntry { path, entry_type } => {
                write!(
                    formatter,
                    "archive entry `{path}` is not a regular file ({entry_type})"
                )
            }
            Self::DuplicateArchivePath(path) => {
                write!(formatter, "archive contains duplicate path {path:?}")
            }
            Self::UnexpectedArchivePath(path) => {
                write!(
                    formatter,
                    "archive path is not listed in the manifest: {path:?}"
                )
            }
            Self::MissingArchivePath(path) => {
                write!(
                    formatter,
                    "manifest file is missing from the archive: {path:?}"
                )
            }
            Self::EntrySizeMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "archive entry {path:?} has {actual} bytes, expected {expected}"
            ),
            Self::ManifestSizeMismatch {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "archive entry {path:?} declares {actual} bytes, expected {expected}"
            ),
            Self::MissingFileHash(path) => {
                write!(formatter, "manifest has no SHA-256 for file {path:?}")
            }
            Self::InvalidFileHash(path) => {
                write!(
                    formatter,
                    "manifest has an invalid SHA-256 for file {path:?}"
                )
            }
            Self::FileHashMismatch { path } => {
                write!(
                    formatter,
                    "archive file SHA-256 does not match manifest: {path:?}"
                )
            }
            Self::ExtractedSizeTooLarge { limit } => {
                write!(formatter, "extracted bundle size exceeds {limit} bytes")
            }
            Self::StagingSizeTooLarge { limit } => {
                write!(formatter, "staged bundle size exceeds {limit} bytes")
            }
            Self::DestinationExists(path) => {
                write!(
                    formatter,
                    "extraction destination is not empty or is unsafe: {}",
                    path.display()
                )
            }
            Self::DestinationParentNotDirectory(path) => {
                write!(
                    formatter,
                    "extraction destination parent is not a directory: {}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for BundleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for BundleError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for BundleError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

fn collect_sources(
    entry_point: &Path,
    additional_files: &[PathBuf],
) -> Result<Vec<BundleSource>, BundleError> {
    let paths = std::iter::once(entry_point).chain(additional_files.iter().map(PathBuf::as_path));
    let paths: Vec<&Path> = paths.collect();
    let common_parent = common_parent(&paths)?;
    let mut sources = Vec::with_capacity(paths.len());
    let mut seen = HashMap::with_capacity(paths.len());
    let mut total_size = 0u64;

    for source in paths {
        let metadata = fs::symlink_metadata(source).map_err(BundleError::Io)?;
        if !metadata.file_type().is_file() {
            return Err(BundleError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("bundle source is not a regular file: {}", source.display()),
            )));
        }
        total_size =
            total_size
                .checked_add(metadata.len())
                .ok_or(BundleError::ExtractedSizeTooLarge {
                    limit: MAX_BUNDLE_SIZE,
                })?;
        if total_size > MAX_BUNDLE_SIZE {
            return Err(BundleError::ExtractedSizeTooLarge {
                limit: MAX_BUNDLE_SIZE,
            });
        }
        let relative = if common_parent.as_os_str().is_empty() {
            source
        } else {
            source
                .strip_prefix(&common_parent)
                .map_err(|_| BundleError::UnsafePath(source.to_string_lossy().into_owned()))?
        };
        let bundle_path = bundle_path_from_path(relative)?;
        if bundle_path.as_str() == MANIFEST_PATH {
            return Err(BundleError::UnsafePath(
                "manifest.json is reserved for the generated manifest".into(),
            ));
        }
        if seen.insert(bundle_path.clone(), ()).is_some() {
            return Err(BundleError::DuplicateManifestPath(bundle_path));
        }
        sources.push(BundleSource {
            source: source.to_path_buf(),
            bundle_path,
            size: metadata.len(),
            sha256: hash_source(source, metadata.len())?,
        });
    }

    if !is_markdown_path(&sources[0].bundle_path) {
        return Err(BundleError::EntryPointNotMarkdown(
            sources[0].bundle_path.clone(),
        ));
    }
    Ok(sources)
}

fn common_parent(paths: &[&Path]) -> Result<PathBuf, BundleError> {
    let first_parent = source_parent(paths[0]);
    let absolute = first_parent.is_absolute();
    let mut common = lexical_components(first_parent);
    for path in paths.iter().skip(1) {
        let path_parent = source_parent(path);
        if path_parent.is_absolute() != absolute {
            return Err(BundleError::UnsafePath(
                "bundle sources must all be absolute or all be relative".into(),
            ));
        }
        let other = lexical_components(path_parent);
        let common_len = common
            .iter()
            .zip(other.iter())
            .take_while(|(left, right)| left == right)
            .count();
        common.truncate(common_len);
    }
    if absolute {
        let mut parent = PathBuf::from(Path::new("/"));
        for component in common {
            parent.push(component);
        }
        Ok(parent)
    } else if common.is_empty() {
        Ok(PathBuf::new())
    } else {
        let mut parent = PathBuf::new();
        for component in common {
            parent.push(component);
        }
        Ok(parent)
    }
}

fn source_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(""))
}

fn lexical_components(path: &Path) -> Vec<PathBuf> {
    path.components()
        .filter_map(|component| match component {
            Component::CurDir => None,
            Component::Normal(value) => Some(PathBuf::from(value)),
            Component::ParentDir => Some(PathBuf::from("..")),
            Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
}

fn bundle_path_from_path(path: &Path) -> Result<BundlePath, BundleError> {
    if path.is_absolute() {
        return Err(BundleError::UnsafePath(path.to_string_lossy().into_owned()));
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or_else(|| BundleError::UnsafePath(path.to_string_lossy().into_owned()))?;
                components.push(value);
            }
            _ => return Err(BundleError::UnsafePath(path.to_string_lossy().into_owned())),
        }
    }
    let value = components.join("/");
    if value.len() > MAX_BUNDLE_PATH_BYTES {
        return Err(BundleError::UnsafePath(value));
    }
    BundlePath::new(value.clone()).map_err(|_| BundleError::UnsafePath(value))
}

fn make_manifest(sources: &[BundleSource]) -> Result<Manifest, BundleError> {
    let files = sources
        .iter()
        .map(|source| ManifestFile {
            path: source.bundle_path.clone(),
            size: source.size,
            sha256: Some(source.sha256.clone()),
        })
        .collect();
    Ok(Manifest {
        protocol_version: CURRENT_PROTOCOL_VERSION,
        bundle_format_version: CURRENT_BUNDLE_FORMAT_VERSION,
        entry_point: sources[0].bundle_path.clone(),
        files,
    })
}

fn append_bytes<W: Write>(
    archive: &mut Builder<W>,
    path: &str,
    bytes: &[u8],
) -> Result<(), BundleError> {
    let mut header = Header::new_ustar();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();
    archive
        .append_data(&mut header, path, Cursor::new(bytes))
        .map_err(BundleError::Io)
}

fn append_source<W: Write>(
    archive: &mut Builder<W>,
    source: &BundleSource,
) -> Result<(), BundleError> {
    let mut file = File::open(&source.source).map_err(BundleError::Io)?;
    let mut bounded_file = HashingReader::new((&mut file).take(source.size));
    let mut header = Header::new_ustar();
    header.set_size(source.size);
    header.set_mode(0o644);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();
    archive
        .append_data(&mut header, source.bundle_path.as_str(), &mut bounded_file)
        .map_err(BundleError::Io)?;
    if bounded_file.inner.limit() != 0 {
        return Err(BundleError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "source file changed while bundling: {}",
                source.source.display()
            ),
        )));
    }
    if hex_digest(&bounded_file.finish()) != source.sha256 {
        return Err(BundleError::FileHashMismatch {
            path: source.bundle_path.clone(),
        });
    }
    Ok(())
}

fn process_archive<R: Read>(
    reader: R,
    destination: Option<&Path>,
    staging_limit: Option<u64>,
    mut memory_files: Option<&mut Vec<(BundlePath, Vec<u8>)>>,
    require_hashes: bool,
) -> Result<Manifest, BundleError> {
    let mut counted = CountingReader::new(reader, MAX_ARCHIVE_SIZE);
    let (manifest, expected, seen) = {
        let mut archive = Archive::new(&mut counted);
        let entries = archive.entries().map_err(BundleError::Io)?.raw(true);
        let mut manifest: Option<Manifest> = None;
        let mut expected: HashMap<BundlePath, ManifestFile> = HashMap::new();
        let mut seen = HashMap::new();
        let mut extracted_size = 0u64;
        let mut staged_size = 0u64;

        for entry_result in entries {
            let mut entry = entry_result.map_err(BundleError::Io)?;
            let entry_type = entry.header().entry_type();
            if entry_type != EntryType::Regular {
                return Err(BundleError::UnsupportedEntry {
                    path: String::from_utf8_lossy(entry.path_bytes().as_ref()).into_owned(),
                    entry_type: format_entry_type(entry_type),
                });
            }
            let path = archive_path(&entry)?;
            if seen.insert(path.clone(), ()).is_some() {
                return Err(BundleError::DuplicateArchivePath(path));
            }

            if path.as_str() == MANIFEST_PATH {
                if manifest.is_some() {
                    return Err(BundleError::DuplicateArchivePath(path));
                }
                let declared_size = entry.header().size().map_err(BundleError::Io)?;
                if declared_size > MAX_MANIFEST_SIZE {
                    return Err(BundleError::ManifestTooLarge {
                        limit: MAX_MANIFEST_SIZE,
                    });
                }
                let bytes = read_entry(&mut entry, &path)?;
                if let Some(limit) = staging_limit {
                    staged_size = bytes.len() as u64;
                    if staged_size > limit {
                        return Err(BundleError::StagingSizeTooLarge { limit });
                    }
                }
                let parsed: Manifest = serde_json::from_slice(&bytes)?;
                expected = validate_manifest(&parsed)?;
                if let Some(destination) = destination {
                    write_staged_file(destination, &path, &bytes)?;
                }
                manifest = Some(parsed);
                continue;
            }

            if manifest.is_none() {
                return Err(BundleError::UnsafePath(
                    "manifest.json must be the first archive entry".into(),
                ));
            }
            let expected_file = expected
                .get(&path)
                .ok_or_else(|| BundleError::UnexpectedArchivePath(path.clone()))?;
            let expected_size = expected_file.size;
            let declared_size = entry.header().size().map_err(BundleError::Io)?;
            if declared_size != expected_size {
                return Err(BundleError::ManifestSizeMismatch {
                    path,
                    expected: expected_size,
                    actual: declared_size,
                });
            }
            if let Some(limit) = staging_limit {
                staged_size = staged_size
                    .checked_add(declared_size)
                    .ok_or(BundleError::StagingSizeTooLarge { limit })?;
                if staged_size > limit {
                    return Err(BundleError::StagingSizeTooLarge { limit });
                }
            }
            extracted_size = extracted_size.checked_add(declared_size).ok_or(
                BundleError::ExtractedSizeTooLarge {
                    limit: MAX_BUNDLE_SIZE,
                },
            )?;
            if extracted_size > MAX_BUNDLE_SIZE {
                return Err(BundleError::ExtractedSizeTooLarge {
                    limit: MAX_BUNDLE_SIZE,
                });
            }
            let digest = if let Some(memory_files) = memory_files.as_deref_mut() {
                let bytes = read_entry(&mut entry, &path)?;
                let digest = Sha256::digest(&bytes).to_vec();
                memory_files.push((path.clone(), bytes));
                digest
            } else if let Some(destination) = destination {
                let staged = staged_path(destination, &path)?;
                let mut output = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(staged)
                    .map_err(BundleError::Io)?;
                let mut hashing = HashingWriter::new(&mut output);
                let actual = io::copy(&mut entry, &mut hashing).map_err(BundleError::Io)?;
                hashing.flush().map_err(BundleError::Io)?;
                if actual != declared_size {
                    return Err(BundleError::EntrySizeMismatch {
                        path,
                        expected: declared_size,
                        actual,
                    });
                }
                hashing.finish()
            } else {
                let mut hashing = HashingWriter::new(io::sink());
                let actual = io::copy(&mut entry, &mut hashing).map_err(BundleError::Io)?;
                if actual != declared_size {
                    return Err(BundleError::EntrySizeMismatch {
                        path,
                        expected: declared_size,
                        actual,
                    });
                }
                hashing.finish()
            };
            validate_file_hash(
                &path,
                expected_file.sha256.as_deref(),
                &digest,
                require_hashes,
            )?;
        }
        (manifest, expected, seen)
    };

    let mut trailing = [0u8; 8192];
    loop {
        let read = counted.read(&mut trailing).map_err(BundleError::Io)?;
        if read == 0 {
            break;
        }
        if trailing[..read].iter().any(|byte| *byte != 0) {
            return Err(BundleError::TrailingArchiveData);
        }
    }

    let manifest =
        manifest.ok_or_else(|| BundleError::UnsafePath("bundle has no manifest.json".into()))?;
    for path in expected.keys() {
        if !seen.contains_key(path) {
            return Err(BundleError::MissingArchivePath(path.clone()));
        }
    }
    Ok(manifest)
}

fn validate_manifest(
    manifest: &Manifest,
) -> Result<HashMap<BundlePath, ManifestFile>, BundleError> {
    if !manifest
        .protocol_version
        .is_compatible_with(CURRENT_PROTOCOL_VERSION)
    {
        return Err(BundleError::UnsupportedProtocolVersion {
            received: manifest.protocol_version,
        });
    }
    if manifest.bundle_format_version != CURRENT_BUNDLE_FORMAT_VERSION {
        return Err(BundleError::UnsupportedBundleVersion {
            received: manifest.bundle_format_version,
        });
    }
    if !is_markdown_path(&manifest.entry_point) {
        return Err(BundleError::EntryPointNotMarkdown(
            manifest.entry_point.clone(),
        ));
    }

    let mut expected = HashMap::with_capacity(manifest.files.len());
    let mut total = 0u64;
    for file in &manifest.files {
        if file.path.as_str().len() > MAX_BUNDLE_PATH_BYTES {
            return Err(BundleError::UnsafePath(file.path.as_str().to_owned()));
        }
        if file.path.as_str() == MANIFEST_PATH {
            return Err(BundleError::ManifestListsManifest);
        }
        if let Some(hash) = file.sha256.as_deref() {
            if !valid_sha256(hash) {
                return Err(BundleError::InvalidFileHash(file.path.clone()));
            }
        }
        if expected.insert(file.path.clone(), file.clone()).is_some() {
            return Err(BundleError::DuplicateManifestPath(file.path.clone()));
        }
        total = total
            .checked_add(file.size)
            .ok_or(BundleError::ExtractedSizeTooLarge {
                limit: MAX_BUNDLE_SIZE,
            })?;
        if total > MAX_BUNDLE_SIZE {
            return Err(BundleError::ExtractedSizeTooLarge {
                limit: MAX_BUNDLE_SIZE,
            });
        }
    }
    if !expected.contains_key(&manifest.entry_point) {
        return Err(BundleError::EntryPointMissingFromManifest(
            manifest.entry_point.clone(),
        ));
    }
    Ok(expected)
}

fn hash_source(path: &Path, expected_size: u64) -> Result<String, BundleError> {
    let mut file = File::open(path).map_err(BundleError::Io)?;
    let mut hashing = HashingWriter::new(io::sink());
    let mut bounded = (&mut file).take(expected_size);
    let actual = io::copy(&mut bounded, &mut hashing).map_err(BundleError::Io)?;
    if actual != expected_size {
        return Err(BundleError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("bundle source changed while hashing: {}", path.display()),
        )));
    }
    Ok(hex_digest(&hashing.finish()))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_file_hash(
    path: &BundlePath,
    expected: Option<&str>,
    actual: &[u8],
    require_hash: bool,
) -> Result<(), BundleError> {
    let Some(expected) = expected else {
        return if require_hash {
            Err(BundleError::MissingFileHash(path.clone()))
        } else {
            Ok(())
        };
    };
    if !valid_sha256(expected) {
        return Err(BundleError::InvalidFileHash(path.clone()));
    }
    if hex_digest(actual) != expected {
        return Err(BundleError::FileHashMismatch { path: path.clone() });
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(value, "{byte:02x}");
    }
    value
}

struct HashingWriter<W> {
    inner: W,
    digest: Sha256,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn finish(self) -> Vec<u8> {
        self.digest.finalize().to_vec()
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let amount = self.inner.write(buffer)?;
        self.digest.update(&buffer[..amount]);
        Ok(amount)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct HashingReader<R> {
    inner: R,
    digest: Sha256,
}

impl<R> HashingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    fn finish(self) -> Vec<u8> {
        self.digest.finalize().to_vec()
    }
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let amount = self.inner.read(buffer)?;
        self.digest.update(&buffer[..amount]);
        Ok(amount)
    }
}

fn is_markdown_path(path: &BundlePath) -> bool {
    Path::new(path.as_str())
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn archive_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<BundlePath, BundleError> {
    let path = entry.path_bytes();
    let path = std::str::from_utf8(&path)
        .map_err(|_| BundleError::UnsafePath("archive path is not UTF-8".into()))?;
    let bundle_path =
        BundlePath::new(path.to_owned()).map_err(|_| BundleError::UnsafePath(path.to_owned()))?;
    if bundle_path.as_str().len() > MAX_BUNDLE_PATH_BYTES {
        return Err(BundleError::UnsafePath(path.to_owned()));
    }
    let platform_path = Path::new(bundle_path.as_str());
    if platform_path.is_absolute()
        || platform_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BundleError::UnsafePath(path.to_owned()));
    }
    Ok(bundle_path)
}

fn read_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    path: &BundlePath,
) -> Result<Vec<u8>, BundleError> {
    let expected = entry.header().size().map_err(BundleError::Io)?;
    let mut bytes = Vec::with_capacity(expected as usize);
    let actual = entry.read_to_end(&mut bytes).map_err(BundleError::Io)? as u64;
    if actual != expected {
        return Err(BundleError::EntrySizeMismatch {
            path: path.clone(),
            expected,
            actual,
        });
    }
    Ok(bytes)
}

fn staged_path(destination: &Path, path: &BundlePath) -> Result<PathBuf, BundleError> {
    let relative = Path::new(path.as_str());
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(BundleError::UnsafePath(path.as_str().to_owned()));
    }
    let target = destination.join(relative);
    let parent = target
        .parent()
        .ok_or_else(|| BundleError::UnsafePath(path.as_str().to_owned()))?;
    fs::create_dir_all(parent).map_err(BundleError::Io)?;
    Ok(target)
}

fn write_staged_file(
    destination: &Path,
    path: &BundlePath,
    bytes: &[u8],
) -> Result<(), BundleError> {
    let target = staged_path(destination, path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(BundleError::Io)?;
    file.write_all(bytes).map_err(BundleError::Io)
}

fn format_entry_type(entry_type: EntryType) -> String {
    format!("{entry_type:?}")
}

struct LimitedWriter<W> {
    inner: W,
    written: u64,
    limit: u64,
    exceeded: bool,
}

impl<W> LimitedWriter<W> {
    fn new(inner: W, limit: u64) -> Self {
        Self {
            inner,
            written: 0,
            limit,
            exceeded: false,
        }
    }

    fn exceeded(&self) -> bool {
        self.exceeded
    }
}

impl<W: Write> Write for LimitedWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let length = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if length > self.limit.saturating_sub(self.written) {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "bundle archive exceeds configured limit",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written = self.written.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct CountingReader<R> {
    inner: R,
    read: u64,
    limit: u64,
    exceeded: bool,
}

impl<R> CountingReader<R> {
    fn new(inner: R, limit: u64) -> Self {
        Self {
            inner,
            read: 0,
            limit,
            exceeded: false,
        }
    }
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.read >= self.limit {
            let mut probe = [0u8; 1];
            let read = self.inner.read(&mut probe)?;
            if read != 0 {
                self.exceeded = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bundle archive exceeds configured limit",
                ));
            }
            return Ok(0);
        }
        let remaining = (self.limit - self.read) as usize;
        let amount = buffer.len().min(remaining);
        let read = self.inner.read(&mut buffer[..amount])?;
        self.read = self.read.saturating_add(read as u64);
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_file(root: &Path, path: &str, contents: &[u8]) -> PathBuf {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, contents).unwrap();
        path
    }

    fn bundle_fixture() -> (TempDir, PathBuf, PathBuf, Vec<u8>) {
        let root = temp_dir();
        let entry = write_file(root.path(), "docs/index.md", b"# Index\n");
        let chapter = write_file(root.path(), "docs/chapters/one.md", b"# One\n");
        let image = write_file(root.path(), "docs/images/diagram.png", b"png bytes");
        // Exercise the explicit-file API with a second, equivalent archive.
        let mut explicit = BundleBuilder::new(&entry);
        explicit.add_files([chapter, image]);
        let mut archive = Vec::new();
        explicit.write(&mut archive).unwrap();
        let source_root = root.path().join("docs");
        (root, entry, source_root, archive)
    }

    #[test]
    fn generated_manifest_and_paths_round_trip() {
        let (_root, _entry, source_root, archive) = bundle_fixture();
        let manifest = validate(Cursor::new(&archive)).unwrap().into_manifest();
        assert_eq!(manifest.entry_point.as_str(), "index.md");
        assert_eq!(
            manifest
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["index.md", "chapters/one.md", "images/diagram.png"]
        );

        let output_parent = temp_dir();
        let output = output_parent.path().join("library");
        let extracted = extract(Cursor::new(&archive), &output).unwrap();
        assert_eq!(extracted, manifest);
        assert_eq!(fs::read(output.join("index.md")).unwrap(), b"# Index\n");
        assert_eq!(
            fs::read(output.join("chapters/one.md")).unwrap(),
            fs::read(source_root.join("chapters/one.md")).unwrap()
        );
        assert_eq!(
            fs::read(output.join(MANIFEST_PATH)).unwrap(),
            serde_json::to_vec(&manifest).unwrap()
        );
    }

    #[test]
    fn in_memory_extraction_requires_and_checks_sha256_before_returning_files() {
        let (_root, _entry, _source_root, archive) = bundle_fixture();
        let extracted = extract_to_memory(Cursor::new(&archive)).unwrap();
        assert_eq!(extracted.manifest.files.len(), 3);
        assert!(extracted
            .manifest
            .files
            .iter()
            .all(|file| file.sha256.as_deref().is_some_and(valid_sha256)));
        assert_eq!(
            extracted
                .files
                .iter()
                .find(|(path, _)| path.as_str() == "index.md")
                .unwrap()
                .1,
            b"# Index\n"
        );

        let path = BundlePath::new("index.md").unwrap();
        let manifest = Manifest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            bundle_format_version: CURRENT_BUNDLE_FORMAT_VERSION,
            entry_point: path.clone(),
            files: vec![ManifestFile {
                path,
                size: 7,
                sha256: Some(hex_digest(&Sha256::digest(b"correct"))),
            }],
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let tampered = tar_with_entries(&[
            (MANIFEST_PATH, EntryType::Regular, &manifest_bytes),
            ("index.md", EntryType::Regular, b"changed"),
        ]);
        assert!(matches!(
            extract_to_memory(Cursor::new(tampered)),
            Err(BundleError::FileHashMismatch { .. })
        ));
    }

    #[test]
    fn in_memory_extraction_rejects_legacy_manifest_without_file_hashes() {
        let manifest_bytes = manifest_bytes(&[("index.md", 5)], "index.md");
        let archive = tar_with_entries(&[
            (MANIFEST_PATH, EntryType::Regular, &manifest_bytes),
            ("index.md", EntryType::Regular, b"hello"),
        ]);
        assert!(matches!(
            extract_to_memory(Cursor::new(archive)),
            Err(BundleError::MissingFileHash(_))
        ));
    }

    #[test]
    fn generated_archive_starts_with_exact_manifest_then_explicit_files() {
        let root = temp_dir();
        let entry = write_file(root.path(), "docs/index.md", b"index");
        let asset = write_file(root.path(), "docs/assets/data.bin", b"asset");
        let mut builder = BundleBuilder::new(entry);
        builder.add_file(asset);
        let mut archive_bytes = Vec::new();
        let manifest = builder.write(&mut archive_bytes).unwrap();

        let mut archive = Archive::new(Cursor::new(archive_bytes));
        let mut entries = archive.entries().unwrap();
        let mut manifest_entry = entries.next().unwrap().unwrap();
        assert_eq!(
            manifest_entry.path_bytes().as_ref(),
            MANIFEST_PATH.as_bytes()
        );
        let mut manifest_bytes = Vec::new();
        manifest_entry.read_to_end(&mut manifest_bytes).unwrap();
        assert_eq!(manifest_bytes, serde_json::to_vec(&manifest).unwrap());
        let entry_path = entries.next().unwrap().unwrap().path_bytes().into_owned();
        let asset_path = entries.next().unwrap().unwrap().path_bytes().into_owned();
        assert_eq!(entry_path, b"index.md");
        assert_eq!(asset_path, b"assets/data.bin");
        assert!(entries.next().is_none());
    }

    #[test]
    fn common_parent_stripping_preserves_nested_paths() {
        let root = temp_dir();
        let entry = write_file(root.path(), "docs/index.md", b"index");
        let chapter = write_file(root.path(), "docs/chapters/one.md", b"one");
        let mut archive = Vec::new();
        let mut builder = BundleBuilder::new(entry);
        builder.add_file(chapter);
        builder.write(&mut archive).unwrap();
        let manifest = validate(Cursor::new(archive)).unwrap().into_manifest();
        assert_eq!(manifest.entry_point.as_str(), "index.md");
        assert_eq!(manifest.files[1].path.as_str(), "chapters/one.md");
    }

    #[test]
    fn generated_long_paths_use_only_regular_ustar_entries() {
        let root = temp_dir();
        let entry = write_file(root.path(), "docs/index.md", b"index");
        let long_relative = format!("{}asset.bin", "nested/".repeat(20));
        let asset = write_file(root.path(), &format!("docs/{long_relative}"), b"asset");
        let mut archive_bytes = Vec::new();
        let mut builder = BundleBuilder::new(entry);
        builder.add_file(asset);
        builder.write(&mut archive_bytes).unwrap();

        let manifest = validate(Cursor::new(&archive_bytes))
            .unwrap()
            .into_manifest();
        assert!(manifest
            .files
            .iter()
            .any(|file| file.path.as_str() == long_relative));

        let mut archive = Archive::new(Cursor::new(archive_bytes));
        for entry in archive.entries().unwrap().raw(true) {
            assert_eq!(entry.unwrap().header().entry_type(), EntryType::Regular);
        }
    }

    #[test]
    fn duplicate_bundle_paths_are_rejected() {
        let root = temp_dir();
        let first = write_file(root.path(), "docs/index.md", b"one");
        let second = write_file(root.path(), "docs/./index.md", b"two");
        let mut builder = BundleBuilder::new(first);
        builder.add_file(second);
        assert!(matches!(
            builder.write(Vec::new()),
            Err(BundleError::DuplicateManifestPath(_))
        ));
    }

    #[test]
    fn entry_point_must_be_markdown() {
        let root = temp_dir();
        let entry = write_file(root.path(), "docs/index.txt", b"text");
        assert!(matches!(
            BundleBuilder::new(entry).write(Vec::new()),
            Err(BundleError::EntryPointNotMarkdown(_))
        ));
    }

    fn tar_with_entries(entries: &[(&str, EntryType, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut builder = Builder::new(&mut bytes);
        for (path, entry_type, contents) in entries {
            let mut header = Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_entry_type(*entry_type);
            header.set_cksum();
            builder
                .append_data(&mut header, *path, Cursor::new(*contents))
                .unwrap();
        }
        builder.finish().unwrap();
        drop(builder);
        bytes
    }

    fn tar_with_raw_paths(entries: &[(&[u8], EntryType, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut builder = Builder::new(&mut bytes);
        for (path, entry_type, contents) in entries {
            assert!(path.len() <= 100);
            let mut header = Header::new_gnu();
            header.as_mut_bytes()[..path.len()].copy_from_slice(path);
            header.set_size(contents.len() as u64);
            header.set_entry_type(*entry_type);
            header.set_cksum();
            builder.append(&header, Cursor::new(*contents)).unwrap();
        }
        builder.finish().unwrap();
        drop(builder);
        bytes
    }

    fn manifest_bytes(files: &[(&str, u64)], entry_point: &str) -> Vec<u8> {
        let manifest = Manifest {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            bundle_format_version: CURRENT_BUNDLE_FORMAT_VERSION,
            entry_point: BundlePath::new(entry_point).unwrap(),
            files: files
                .iter()
                .map(|(path, size)| ManifestFile {
                    path: BundlePath::new(*path).unwrap(),
                    size: *size,
                    sha256: None,
                })
                .collect(),
        };
        serde_json::to_vec(&manifest).unwrap()
    }

    #[test]
    fn unsafe_paths_and_special_entries_are_rejected_before_destination_visibility() {
        for path in [
            "/index.md",
            "../index.md",
            "chapter/../index.md",
            "chapter\\index.md",
        ] {
            // Paths rejected by BundlePath cannot be represented in a normal
            // manifest, so use a valid manifest and alter the tar path.
            let valid_manifest = manifest_bytes(&[("index.md", 1)], "index.md");
            let archive = tar_with_raw_paths(&[
                (
                    MANIFEST_PATH.as_bytes(),
                    EntryType::Regular,
                    &valid_manifest,
                ),
                (path.as_bytes(), EntryType::Regular, b"x"),
            ]);
            let output_parent = temp_dir();
            let output = output_parent.path().join("out");
            assert!(validate(Cursor::new(archive)).is_err());
            assert!(!output.exists());
        }

        let valid_manifest = manifest_bytes(&[("index.md", 1)], "index.md");
        for entry_type in [
            EntryType::Symlink,
            EntryType::Link,
            EntryType::Char,
            EntryType::Block,
            EntryType::Fifo,
            EntryType::Directory,
            EntryType::Continuous,
        ] {
            let archive = tar_with_entries(&[
                (MANIFEST_PATH, EntryType::Regular, &valid_manifest),
                ("index.md", entry_type, b"target"),
            ]);
            assert!(matches!(
                validate(Cursor::new(archive)),
                Err(BundleError::UnsupportedEntry { .. })
            ));
        }
    }

    #[test]
    fn pax_extension_members_are_rejected() {
        let valid_manifest = manifest_bytes(&[("index.md", 1)], "index.md");
        let mut archive = Vec::new();
        let mut builder = Builder::new(&mut archive);
        append_bytes(&mut builder, MANIFEST_PATH, &valid_manifest).unwrap();
        builder
            .append_pax_extensions([("comment", b"metadata".as_slice())])
            .unwrap();
        append_bytes(&mut builder, "index.md", b"x").unwrap();
        builder.finish().unwrap();
        drop(builder);

        assert!(matches!(
            validate(Cursor::new(archive)),
            Err(BundleError::UnsupportedEntry { .. })
        ));
    }

    #[test]
    fn gnu_long_name_members_are_rejected() {
        let long_path = format!("{}index.md", "nested/".repeat(20));
        let valid_manifest =
            manifest_bytes(&[("index.md", 1), (long_path.as_str(), 1)], "index.md");
        let mut archive = Vec::new();
        let mut builder = Builder::new(&mut archive);
        append_bytes(&mut builder, MANIFEST_PATH, &valid_manifest).unwrap();
        append_bytes(&mut builder, "index.md", b"x").unwrap();

        let mut header = Header::new_gnu();
        header.set_size(1);
        header.set_entry_type(EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, &long_path, Cursor::new(b"x"))
            .unwrap();
        builder.finish().unwrap();
        drop(builder);

        assert!(matches!(
            validate(Cursor::new(archive)),
            Err(BundleError::UnsupportedEntry { .. })
        ));
    }

    #[test]
    fn invalid_archive_never_publishes_staged_files() {
        let valid_manifest = manifest_bytes(&[("index.md", 1)], "index.md");
        let archive = tar_with_entries(&[
            (MANIFEST_PATH, EntryType::Regular, &valid_manifest),
            ("index.md", EntryType::Regular, b"x"),
            ("extra.txt", EntryType::Regular, b"unexpected"),
        ]);
        let output_parent = temp_dir();
        let output = output_parent.path().join("out");
        assert!(matches!(
            extract(Cursor::new(archive), &output),
            Err(BundleError::UnexpectedArchivePath(_))
        ));
        assert!(!output.exists());
    }

    #[cfg(unix)]
    #[test]
    fn extraction_rejects_symlink_destination() {
        let root = temp_dir();
        let entry = write_file(root.path(), "index.md", b"content");
        let mut archive = Vec::new();
        BundleBuilder::new(entry).write(&mut archive).unwrap();
        let destination = root.path().join("link");
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &destination).unwrap();
        assert!(matches!(
            extract(Cursor::new(archive), &destination),
            Err(BundleError::DestinationExists(_))
        ));
        assert!(fs::read_dir(&target).unwrap().next().is_none());
    }

    #[test]
    fn unsupported_version_and_duplicate_paths_are_rejected() {
        let mut manifest: Manifest =
            serde_json::from_slice(&manifest_bytes(&[("index.md", 1)], "index.md")).unwrap();
        manifest.bundle_format_version = CURRENT_BUNDLE_FORMAT_VERSION + 1;
        let bytes = serde_json::to_vec(&manifest).unwrap();
        let archive = tar_with_entries(&[(MANIFEST_PATH, EntryType::Regular, &bytes)]);
        assert!(matches!(
            validate(Cursor::new(archive)),
            Err(BundleError::UnsupportedBundleVersion { .. })
        ));

        let valid_manifest = manifest_bytes(&[("index.md", 1)], "index.md");
        let archive = tar_with_entries(&[
            (MANIFEST_PATH, EntryType::Regular, &valid_manifest),
            ("index.md", EntryType::Regular, b"x"),
            ("index.md", EntryType::Regular, b"x"),
        ]);
        assert!(matches!(
            validate(Cursor::new(archive)),
            Err(BundleError::DuplicateArchivePath(_))
        ));
    }

    #[test]
    fn archive_size_is_bounded() {
        let mut archive = vec![0u8; MAX_ARCHIVE_SIZE as usize];
        archive.push(1);
        let result = validate(Cursor::new(archive));
        assert!(matches!(
            result,
            Err(BundleError::Io(_)) | Err(BundleError::ArchiveTooLarge { .. })
        ));
    }

    #[test]
    fn extraction_does_not_require_complete_archive_in_memory() {
        let root = temp_dir();
        let entry = write_file(root.path(), "index.md", b"streamed");
        let mut archive = Vec::new();
        BundleBuilder::new(entry).write(&mut archive).unwrap();
        let output_parent = temp_dir();
        let output = output_parent.path().join("out");
        let reader = ChunkedReader::new(archive, 7);
        extract(reader, &output).unwrap();
        assert_eq!(fs::read(output.join("index.md")).unwrap(), b"streamed");
    }

    #[test]
    fn extraction_size_limit_counts_raw_manifest_bytes() {
        let manifest = manifest_bytes(&[("index.md", 1)], "index.md");
        let padded_manifest = [manifest.as_slice(), b"  \n"].concat();
        let archive = tar_with_entries(&[
            (MANIFEST_PATH, EntryType::Regular, &padded_manifest),
            ("index.md", EntryType::Regular, b"x"),
        ]);
        let output_parent = temp_dir();
        let output = output_parent.path().join("out");
        let limit = manifest.len() as u64 + 1;

        assert!(matches!(
            extract_with_size_limit(Cursor::new(archive), &output, limit),
            Err(BundleError::StagingSizeTooLarge { .. })
        ));
        assert!(!output.exists());
    }

    struct ChunkedReader {
        bytes: Vec<u8>,
        offset: usize,
        chunk: usize,
    }

    impl ChunkedReader {
        fn new(bytes: Vec<u8>, chunk: usize) -> Self {
            Self {
                bytes,
                offset: 0,
                chunk,
            }
        }
    }

    impl Read for ChunkedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.offset == self.bytes.len() {
                return Ok(0);
            }
            let count = self
                .chunk
                .min(buffer.len())
                .min(self.bytes.len() - self.offset);
            buffer[..count].copy_from_slice(&self.bytes[self.offset..self.offset + count]);
            self.offset += count;
            Ok(count)
        }
    }
}
