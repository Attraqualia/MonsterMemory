//! Versioned checkpoint and write-ahead-log containers for `MonsterMemory`.
//!
//! An Attraqualia project, developed by halnos.
//!
//! Generated database facades use [`DatabaseImage`] to store table payloads. This
//! crate can also serialize any Serde value with [`write_checkpoint`] and recover
//! it with [`read_checkpoint`]. Containers carry a caller-supplied schema
//! fingerprint and CRC32 checksum; the fingerprint is not inferred from `T`.
//!
//! # Table payloads
//!
//! ```
//! use monstermemory_persistence::{decode_table, encode_table};
//!
//! let rows = vec![1_u64, 2, 3];
//! let payload = encode_table(&rows)?;
//! let restored: Vec<u64> = decode_table(&payload)?;
//! assert_eq!(restored, rows);
//! # Ok::<(), monstermemory_persistence::Error>(())
//! ```
//!
//! # Limits and coordination
//!
//! Checkpoint replacement is atomic through `atomic-write-file`; WAL append and
//! deletion are separate operations. Direct callers must coordinate concurrent
//! writers and checkpoint/WAL ordering. See [`Durability`] for the synchronization
//! actually requested by each operation.
//!
//! Readers limit the encoded payload buffer to 1 GiB by default; use
//! [`read_checkpoint_with_limit`] or [`read_latest_wal_with_limit`] for another
//! limit. Deserialization can allocate additional memory, so this is not a bound
//! on total memory use. CRC32 detects accidental corruption, not malicious edits.
//! The alpha format has no migration or cross-version compatibility guarantee.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    reason = "the closed persistence Error enum documents every failure category"
)]
#![allow(
    clippy::missing_panics_doc,
    reason = "fixed-size checked header slices make the reported expect paths unreachable"
)]

use atomic_write_file::AtomicWriteFile;
use crc32fast::Hasher;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const MAGIC: &[u8; 8] = b"MONMEM\0\x01";
const WAL_MAGIC: &[u8; 8] = b"MONWAL\0\x01";
const COMMIT_MARKER: &[u8; 4] = b"CMIT";
const HEADER_LENGTH: usize = 8 + 2 + 2 + 8 + 8 + 4;
const WAL_HEADER_LENGTH: usize = 8 + 2 + 2 + 8;
const WAL_FRAME_OVERHEAD: usize = 8 + 4 + 4;
const FORMAT_VERSION: u16 = 1;
const DEFAULT_LIMIT: u64 = 1024 * 1024 * 1024;

/// Result type for stable-container operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Failures while encoding, validating, or accessing a persistence container.
///
/// Container lengths are checked before allocating the encoded payload buffer.
/// Deserialization errors use [`Self::Encode`] for both reads and writes.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The checkpoint or WAL fingerprint differs from the expected fingerprint.
    #[error("schema mismatch (expected {expected:#018x}, found {actual:#018x})")]
    SchemaMismatch {
        /// Compiled fingerprint.
        expected: u64,
        /// Stored fingerprint.
        actual: u64,
    },
    /// A container header or complete frame is invalid, oversized, or corrupted.
    #[error("corrupt checkpoint at byte {offset}: {message}")]
    CorruptData {
        /// Byte offset associated with the problem.
        offset: u64,
        /// Concise explanation.
        message: String,
    },
    /// An operating-system I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The portable payload could not be serialized or deserialized as requested.
    #[error("checkpoint encoding failed: {0}")]
    Encode(#[from] postcard::Error),
}

/// Synchronization policy for checkpoint writes and WAL appends.
///
/// All writes call `flush`. [`Self::Async`] and [`Self::Flush`] currently take the
/// same path. Checkpoint replacement additionally synchronizes its temporary file
/// inside `atomic-write-file` for every policy; [`Self::Fsync`] requests an extra
/// `sync_all` before replacement. For WAL appends, only [`Self::Fsync`] requests
/// `sync_all`. These requests are subject to filesystem and device guarantees;
/// this API does not explicitly synchronize parent-directory metadata.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Durability {
    /// Flush writes without an additional explicit `sync_all` request.
    #[default]
    Async,
    /// Currently performs the same operations as [`Self::Async`].
    Flush,
    /// Explicitly synchronize the written file's contents and metadata.
    Fsync,
}

/// Portable database payload used by generated facades.
///
/// Table order must match the generated schema. This type does not validate table
/// counts or decode individual row vectors; generated facades perform that work.
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseImage {
    /// One postcard-encoded row vector per generated table, in stable table-ID order.
    pub tables: Vec<Vec<u8>>,
}

/// Encodes one table's rows without exposing the serialization implementation.
///
/// This serializes the supplied value as a postcard payload without a container
/// header, checksum, schema fingerprint, or size limit. See the crate example for
/// a round trip with [`decode_table`].
///
/// # Errors
///
/// Returns [`Error::Encode`] if Serde/postcard cannot serialize `rows`.
pub fn encode_table<T: Serialize>(rows: &T) -> Result<Vec<u8>> {
    Ok(postcard::to_stdvec(rows)?)
}

/// Deserializes a postcard table payload into an owned value.
///
/// This helper does not check a checksum, schema fingerprint, or size limit.
/// Generated facades use it after validating the outer container. Direct callers
/// must establish any required input limits themselves; decoding may allocate.
///
/// # Errors
///
/// Returns [`Error::Encode`] if `bytes` cannot be decoded as `T`.
pub fn decode_table<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    postcard::from_bytes(bytes).map_err(Error::Encode)
}

impl From<Error> for monstermemory_core::Error {
    fn from(error: Error) -> Self {
        match error {
            Error::SchemaMismatch { expected, actual } => Self::SchemaMismatch { expected, actual },
            Error::CorruptData { offset, message } => Self::CorruptData {
                offset,
                section: "checkpoint",
                message,
            },
            Error::Io(error) => Self::Io(error),
            Error::Encode(error) => Self::CorruptData {
                offset: 0,
                section: "payload",
                message: error.to_string(),
            },
        }
    }
}

/// Atomically writes a versioned, checksummed checkpoint.
///
/// Serializes all of `value` in memory before opening a temporary file beside
/// `path`, then replaces the destination after the complete container is written.
/// The parent directory must already exist. `schema_fingerprint` is stored
/// verbatim and must be supplied again when reading.
///
/// The destination changes at replacement; a path that is a symbolic link is
/// replaced rather than followed. See [`Durability`] for synchronization behavior.
/// This operation does not append to or clear a WAL.
///
/// # Examples
///
/// ```
/// use monstermemory_persistence::{Durability, read_checkpoint, write_checkpoint};
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let nonce = std::time::SystemTime::now()
///     .duration_since(std::time::UNIX_EPOCH)?.as_nanos();
/// let directory = std::env::temp_dir()
///     .join(format!("monstermemory-example-{}-{nonce}", std::process::id()));
/// std::fs::create_dir(&directory)?;
/// let path = directory.join("snapshot.monmem");
/// write_checkpoint(&path, 42, &vec![1_u64, 2, 3], Durability::Fsync)?;
/// let restored: Vec<u64> = read_checkpoint(&path, 42)?;
/// assert_eq!(restored, [1, 2, 3]);
/// std::fs::remove_file(path)?;
/// std::fs::remove_dir(directory)?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns [`Error::Encode`] for serialization failure, [`Error::CorruptData`]
/// if the encoded length cannot fit in `u64`, or [`Error::Io`] if creating,
/// writing, flushing, synchronizing, or replacing the file fails.
pub fn write_checkpoint<T: Serialize>(
    path: impl AsRef<Path>,
    schema_fingerprint: u64,
    value: &T,
    durability: Durability,
) -> Result<()> {
    let path = path.as_ref();
    let payload = postcard::to_stdvec(value)?;
    let payload_len = u64::try_from(payload.len()).map_err(|_| Error::CorruptData {
        offset: 0,
        message: "encoded payload exceeds u64 length".to_owned(),
    })?;
    let checksum = crc32fast::hash(&payload);
    let mut file = AtomicWriteFile::options().open(path)?;
    file.write_all(MAGIC)?;
    file.write_all(&FORMAT_VERSION.to_le_bytes())?;
    file.write_all(&0_u16.to_le_bytes())?;
    file.write_all(&schema_fingerprint.to_le_bytes())?;
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&checksum.to_le_bytes())?;
    file.write_all(&payload)?;
    file.flush()?;
    if durability == Durability::Fsync {
        file.sync_all()?;
    }
    // commit itself synchronizes the temporary file, so Async/Flush do not skip
    // checkpoint synchronization; they only omit the explicit sync_all above.
    file.commit()?;
    Ok(())
}

/// Opens a checkpoint with a maximum encoded payload size of 1 GiB.
///
/// Equivalent to [`read_checkpoint_with_limit`] with a limit of `1_073_741_824`
/// bytes. Deserializes as `T` only after container validation. See
/// [`write_checkpoint`] for a round-trip example.
///
/// # Errors
///
/// Returns the same errors as [`read_checkpoint_with_limit`]. A missing file is
/// an [`Error::Io`], rather than an empty database.
pub fn read_checkpoint<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    expected_schema: u64,
) -> Result<T> {
    read_checkpoint_with_limit(path, expected_schema, DEFAULT_LIMIT)
}

/// Opens and validates a checkpoint with an explicit maximum payload size.
///
/// The limit is inclusive and applies only to encoded payload bytes, excluding
/// the 32-byte header and any allocations made during deserialization. Magic,
/// format version, schema fingerprint, exact file length, and CRC32 are validated
/// before deserializing. Both truncated payloads and trailing file bytes fail.
/// The file must remain unchanged while it is read.
///
/// # Errors
///
/// Returns [`Error::SchemaMismatch`] for a differing fingerprint;
/// [`Error::CorruptData`] for an invalid header, unsupported version, excessive
/// or unrepresentable payload size, length mismatch, or checksum mismatch;
/// [`Error::Encode`] if the validated payload cannot be decoded as `T`; or
/// [`Error::Io`] if opening, inspecting, or reading the file fails.
pub fn read_checkpoint_with_limit<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    expected_schema: u64,
    maximum_payload_bytes: u64,
) -> Result<T> {
    let mut file = fs::File::open(path)?;
    let file_len = file.metadata()?.len();
    if file_len < HEADER_LENGTH as u64 {
        return Err(corrupt(0, "file is shorter than the checkpoint header"));
    }
    let mut header = [0_u8; HEADER_LENGTH];
    file.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(corrupt(0, "invalid magic bytes"));
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != FORMAT_VERSION {
        return Err(corrupt(8, format!("unsupported format version {version}")));
    }
    let schema = u64::from_le_bytes(header[12..20].try_into().expect("fixed header slice"));
    if schema != expected_schema {
        return Err(Error::SchemaMismatch {
            expected: expected_schema,
            actual: schema,
        });
    }
    let payload_len = u64::from_le_bytes(header[20..28].try_into().expect("fixed header slice"));
    if payload_len > maximum_payload_bytes {
        return Err(corrupt(20, "payload exceeds the configured size limit"));
    }
    let expected_len = (HEADER_LENGTH as u64)
        .checked_add(payload_len)
        .ok_or_else(|| corrupt(20, "payload length overflow"))?;
    if expected_len != file_len {
        return Err(corrupt(20, "payload length does not match the file length"));
    }
    let expected_checksum =
        u32::from_le_bytes(header[28..32].try_into().expect("fixed header slice"));
    let allocation = usize::try_from(payload_len)
        .map_err(|_| corrupt(20, "payload cannot be represented on this platform"))?;
    // Bound the encoded buffer before allocating; Serde's allocations are a
    // separate concern and are not constrained by maximum_payload_bytes.
    let mut payload = vec![0_u8; allocation];
    file.read_exact(&mut payload)?;
    let mut hasher = Hasher::new();
    hasher.update(&payload);
    if hasher.finalize() != expected_checksum {
        return Err(corrupt(28, "payload checksum mismatch"));
    }
    postcard::from_bytes(&payload).map_err(Error::Encode)
}

/// Appends one checksummed, commit-marked recovery frame.
///
/// Creates a WAL header if the file does not exist; an existing file must already
/// have a complete, matching header. Each frame stores all of `value`, not a
/// record-level delta. The parent directory must exist, and no write-size limit
/// is imposed. See [`read_latest_wal`] for recovery.
///
/// Direct callers must serialize appends, checkpoint replacement, and WAL
/// deletion. This function validates only the existing header: it neither repairs
/// a torn tail nor validates earlier frames before appending. An I/O failure may
/// leave a partial frame. Do not append after an unrepaired tail.
///
/// # Errors
///
/// Returns [`Error::SchemaMismatch`] for an existing WAL with a different schema,
/// [`Error::CorruptData`] for an invalid/truncated header or an encoded length
/// that cannot fit in `u64`, [`Error::Encode`] for serialization failure, or
/// [`Error::Io`] for file inspection, open, write, flush, or synchronization failure.
pub fn append_wal<T: Serialize>(
    path: impl AsRef<Path>,
    schema_fingerprint: u64,
    value: &T,
    durability: Durability,
) -> Result<()> {
    let path = path.as_ref();
    let payload = postcard::to_stdvec(value)?;
    let payload_len =
        u64::try_from(payload.len()).map_err(|_| corrupt(0, "WAL payload exceeds u64 length"))?;
    let checksum = crc32fast::hash(&payload);
    let exists = path.try_exists()?;
    if exists {
        validate_wal_header(path, schema_fingerprint)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if !exists || file.metadata()?.len() == 0 {
        file.write_all(WAL_MAGIC)?;
        file.write_all(&FORMAT_VERSION.to_le_bytes())?;
        file.write_all(&0_u16.to_le_bytes())?;
        file.write_all(&schema_fingerprint.to_le_bytes())?;
    }
    file.write_all(&payload_len.to_le_bytes())?;
    file.write_all(&checksum.to_le_bytes())?;
    file.write_all(&payload)?;
    file.write_all(COMMIT_MARKER)?;
    file.flush()?;
    if durability == Durability::Fsync {
        file.sync_all()?;
    }
    Ok(())
}

/// Returns the last complete WAL value with a 1 GiB encoded size limit per frame.
///
/// Equivalent to [`read_latest_wal_with_limit`] with a `1_073_741_824` byte
/// per-frame limit. A missing file or a valid header without complete frames
/// returns `Ok(None)`. Earlier frames are decoded in order; only the last value
/// is returned. Reading never repairs or truncates the file.
///
/// # Errors
///
/// Returns the same errors as [`read_latest_wal_with_limit`].
pub fn read_latest_wal<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    expected_schema: u64,
) -> Result<Option<T>> {
    read_latest_wal_with_limit(path, expected_schema, DEFAULT_LIMIT)
}

/// Replays the last WAL frame with an explicit per-frame size limit.
///
/// `maximum_frame_bytes` is an inclusive limit on each frame's encoded payload,
/// excluding its 16-byte overhead and Serde allocations. Complete frames must
/// have a valid marker, CRC32, and payload of type `T`. A missing file returns
/// `Ok(None)`; a valid header with no complete frames does likewise.
///
/// An incomplete trailing frame is ignored, provided its declared length has not
/// already exceeded the configured limit or overflowed. A complete corrupt frame
/// is an error even if an earlier frame was valid; recovery does not fall back.
/// There is no limit on the number of frames or total bytes scanned. The file
/// must remain unchanged while it is read.
///
/// # Errors
///
/// Returns [`Error::SchemaMismatch`] for a differing fingerprint;
/// [`Error::CorruptData`] for an invalid/truncated header, unsupported version,
/// excessive or unrepresentable frame size, length overflow, invalid commit
/// marker, or checksum mismatch; [`Error::Encode`] if any complete frame cannot
/// be decoded as `T`; or [`Error::Io`] for other open, metadata, or read failures.
pub fn read_latest_wal_with_limit<T: DeserializeOwned>(
    path: impl AsRef<Path>,
    expected_schema: u64,
    maximum_frame_bytes: u64,
) -> Result<Option<T>> {
    let path = path.as_ref();
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Io(error)),
    };
    let file_len = file.metadata()?.len();
    if file_len < WAL_HEADER_LENGTH as u64 {
        return Err(corrupt(0, "WAL is shorter than its header"));
    }
    let mut header = [0_u8; WAL_HEADER_LENGTH];
    file.read_exact(&mut header)?;
    validate_wal_header_bytes(&header, expected_schema)?;
    let mut offset = WAL_HEADER_LENGTH as u64;
    let mut latest = None;
    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < WAL_FRAME_OVERHEAD as u64 {
            break;
        }
        let mut frame_header = [0_u8; 12];
        file.read_exact(&mut frame_header)?;
        let payload_len = u64::from_le_bytes(
            frame_header[..8]
                .try_into()
                .expect("fixed frame header slice"),
        );
        let checksum = u32::from_le_bytes(
            frame_header[8..12]
                .try_into()
                .expect("fixed frame header slice"),
        );
        if payload_len > maximum_frame_bytes {
            return Err(corrupt(
                offset,
                "WAL frame exceeds the configured size limit",
            ));
        }
        let complete_len = 12_u64
            .checked_add(payload_len)
            .and_then(|length| length.checked_add(4))
            .ok_or_else(|| corrupt(offset, "WAL frame length overflow"))?;
        // A torn final append is recoverable; a complete frame with a bad marker
        // or checksum is corruption and must not silently fall back.
        if remaining < complete_len {
            break;
        }
        let allocation = usize::try_from(payload_len)
            .map_err(|_| corrupt(offset, "WAL frame cannot fit on this platform"))?;
        let mut payload = vec![0_u8; allocation];
        file.read_exact(&mut payload)?;
        let mut marker = [0_u8; 4];
        file.read_exact(&mut marker)?;
        if &marker != COMMIT_MARKER {
            return Err(corrupt(
                offset + 12 + payload_len,
                "invalid WAL commit marker",
            ));
        }
        if crc32fast::hash(&payload) != checksum {
            return Err(corrupt(offset + 8, "WAL frame checksum mismatch"));
        }
        latest = Some(postcard::from_bytes(&payload).map_err(Error::Encode)?);
        offset += complete_len;
    }
    Ok(latest)
}

/// Removes a WAL after a checkpoint has durably captured its latest frame.
///
/// A missing file is treated as success. The caller must establish that the WAL
/// is no longer needed and exclude concurrent appenders; this function does not
/// inspect the checkpoint, lock the WAL, or synchronize its parent directory.
///
/// # Errors
///
/// Returns [`Error::Io`] if removal fails for a reason other than a missing file.
pub fn clear_wal(path: impl AsRef<Path>) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::Io(error)),
    }
}

/// Derives the sidecar WAL path without interpreting user-controlled components.
///
/// Appends `.wal` to the complete path, preserving any existing extension. It
/// performs no filesystem access or path normalization.
///
/// # Examples
///
/// ```
/// use monstermemory_persistence::wal_path;
/// assert_eq!(wal_path("snapshot.bin"), std::path::PathBuf::from("snapshot.bin.wal"));
/// ```
#[must_use]
pub fn wal_path(checkpoint: impl AsRef<Path>) -> std::path::PathBuf {
    let mut path = checkpoint.as_ref().as_os_str().to_owned();
    path.push(".wal");
    path.into()
}

fn validate_wal_header(path: &Path, expected_schema: u64) -> Result<()> {
    let mut file = fs::File::open(path)?;
    let mut header = [0_u8; WAL_HEADER_LENGTH];
    file.read_exact(&mut header)
        .map_err(|_| corrupt(0, "WAL header is truncated"))?;
    validate_wal_header_bytes(&header, expected_schema)
}

fn validate_wal_header_bytes(header: &[u8; WAL_HEADER_LENGTH], expected_schema: u64) -> Result<()> {
    if &header[..8] != WAL_MAGIC {
        return Err(corrupt(0, "invalid WAL magic bytes"));
    }
    let version = u16::from_le_bytes([header[8], header[9]]);
    if version != FORMAT_VERSION {
        return Err(corrupt(
            8,
            format!("unsupported WAL format version {version}"),
        ));
    }
    let schema = u64::from_le_bytes(header[12..20].try_into().expect("fixed WAL header slice"));
    if schema != expected_schema {
        return Err(Error::SchemaMismatch {
            expected: expected_schema,
            actual: schema,
        });
    }
    Ok(())
}

fn corrupt(offset: u64, message: impl Into<String>) -> Error {
    Error::CorruptData {
        offset,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Durability, Error, append_wal, read_checkpoint, read_latest_wal, write_checkpoint,
    };
    use std::{
        fs,
        io::Write as _,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn path(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("monstermemory-{name}-{nonce}.bin"))
    }

    #[test]
    fn checkpoint_round_trip_and_schema_validation() {
        let path = path("round-trip");
        write_checkpoint(&path, 42, &vec![1_u64, 2, 3], Durability::Flush).unwrap();
        let restored: Vec<u64> = read_checkpoint(&path, 42).unwrap();
        assert_eq!(restored, [1, 2, 3]);
        assert!(matches!(
            read_checkpoint::<Vec<u64>>(&path, 7),
            Err(Error::SchemaMismatch { .. })
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn checksum_rejects_corruption() {
        let path = path("corrupt");
        write_checkpoint(&path, 42, &"monster", Durability::Async).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            read_checkpoint::<String>(&path, 42),
            Err(Error::CorruptData { .. })
        ));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn wal_replays_last_commit_and_ignores_a_torn_tail() {
        let path = path("wal");
        append_wal(&path, 42, &vec![1_u64], Durability::Flush).unwrap();
        append_wal(&path, 42, &vec![2_u64], Durability::Flush).unwrap();
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(&99_u64.to_le_bytes()).unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap();
        file.write_all(&[1, 2]).unwrap();
        file.flush().unwrap();
        let latest: Vec<u64> = read_latest_wal(&path, 42).unwrap().unwrap();
        assert_eq!(latest, [2]);
        fs::remove_file(path).unwrap();
    }
}
