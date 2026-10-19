//! Archive codecs for `runtime:archive` (DECISIONS D174): tar, tar.gz and zip
//! in and out.
//!
//! Reads are validated before use. Every entry path is refused unless it is a
//! relative, slash-separated UTF-8 path with no `.`/`..` component, no
//! backslash and no drive letter; links and special entries are refused rather
//! than followed; duplicate paths and file/directory collisions are refused.
//! The caller may then write the validated entries, which is what keeps
//! extraction from ever naming a path the archive did not name safely.
//!
//! Writes are deterministic: entries are sorted by path, ownership is `0:0`,
//! modes are `0644` for files and `0755` for directories, and a timestamp is
//! `0` unless the caller supplied one (zip clamps to its 1980 epoch). A build
//! of the same entries therefore produces the same archive.
//!
//! Limits are enforced twice: declared sizes are checked before anything is
//! allocated, and the bytes that actually come out of a decompressor are
//! counted, so a header that lies cannot exceed the budget.

use std::collections::BTreeSet;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{ProviderError, SyncFd, SyncFileSystem, SyncOpenOptions, SyncWhence};
use futures_channel::mpsc;
use futures_util::SinkExt;

/// Default cap on entries in one archive.
pub(crate) const DEFAULT_MAX_ENTRIES: usize = 100_000;
/// Default cap on the uncompressed bytes of one archive (1 GiB).
pub(crate) const DEFAULT_MAX_BYTES: u64 = 1 << 30;

/// A tar stream, with or without its gzip envelope, or a zip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    Tar,
    TarGz,
    Zip,
}

impl Format {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Format::Tar => "tar",
            Format::TarGz => "tar.gz",
            Format::Zip => "zip",
        }
    }
}

/// How a new archive is compressed. `Gzip` belongs to tar and `Deflate` to
/// zip; the other combinations are argument errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Compression {
    None,
    Gzip,
    Deflate,
}

impl Compression {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Compression::None => "none",
            Compression::Gzip => "gzip",
            Compression::Deflate => "deflate",
        }
    }
}

/// The resource bounds one read or build obeys.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    pub max_entries: usize,
    pub max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// A failure in the codec itself. Path/provider errors are the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Error {
    /// The bytes are not a well-formed archive.
    Decode(String),
    /// Well-formed, but a format or method this runtime does not read.
    Unsupported(String),
    /// Encrypted: no password is accepted, by design.
    Encrypted(String),
    /// An entry path that is not safe to extract, or a layout collision.
    UnsafePath(String),
    /// An entry-count or uncompressed-size bound was exceeded.
    TooLarge(String),
    /// A bad argument from the guest (a build entry, an option).
    Type(String),
    /// The named entry is not in the archive.
    NotFound(String),
}

impl Error {
    pub(crate) fn code(&self) -> Option<ErrorCode> {
        match self {
            Error::Decode(_) => Some(ErrorCode::ArchiveDecodeFailed),
            Error::Unsupported(_) => Some(ErrorCode::ArchiveFormatUnsupported),
            Error::Encrypted(_) => Some(ErrorCode::ArchiveEncrypted),
            Error::UnsafePath(_) => Some(ErrorCode::ArchiveUnsafePath),
            Error::TooLarge(_) => Some(ErrorCode::ArchiveTooLarge),
            Error::NotFound(_) => Some(ErrorCode::NotFound),
            Error::Type(_) => None,
        }
    }

    pub(crate) fn class(&self) -> es_runtime_common::ExceptionClass {
        use es_runtime_common::ExceptionClass;
        match self {
            Error::Type(_) => ExceptionClass::TypeError,
            Error::TooLarge(_) => ExceptionClass::RangeError,
            _ => ExceptionClass::Error,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Decode(m)
            | Error::Unsupported(m)
            | Error::Encrypted(m)
            | Error::UnsafePath(m)
            | Error::TooLarge(m)
            | Error::Type(m)
            | Error::NotFound(m) => f.write_str(m),
        }
    }
}

/// One entry's metadata, with the path already validated and normalized (no
/// trailing slash).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EntryMeta {
    pub path: String,
    pub directory: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch, when the archive carries one.
    pub modified_ms: Option<u64>,
}

/// One entry's validated metadata plus its bytes (empty for a directory).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub meta: EntryMeta,
    pub data: Vec<u8>,
}

/// An entry a guest asked to build. `data` is `None` for a directory.
#[derive(Debug, Clone)]
pub(crate) struct InputEntry {
    pub path: String,
    pub data: Option<Vec<u8>>,
    pub modified_ms: Option<u64>,
}

/// One event in a streamed entry read.
#[derive(Debug)]
pub(crate) enum StreamEvent {
    Data(Vec<u8>),
    Done,
    Error(Error),
}

/// One event in a streamed extraction.
#[derive(Debug)]
pub(crate) enum ExtractEvent {
    Entry(EntryMeta),
    Data(Vec<u8>),
    EndEntry,
    Done,
    Error(Error),
}

/// Where an archive's bytes come from. A file source is read in bounded chunks
/// through the synchronous filesystem provider, so listing, streaming and
/// extraction never hold the whole archive in memory.
#[derive(Clone)]
pub(crate) enum Source {
    Bytes(Arc<[u8]>),
    File {
        fs: Arc<dyn SyncFileSystem>,
        path: Arc<str>,
        size: u64,
    },
}

impl Source {
    pub(crate) fn bytes(bytes: Vec<u8>) -> Source {
        Source::Bytes(Arc::from(bytes))
    }

    /// Opens `path` once to verify it is a readable regular file. The handle is
    /// not kept: each reader reopens it, so concurrent reads have their own
    /// cursor and no fd outlives an operation.
    pub(crate) fn file(fs: Arc<dyn SyncFileSystem>, path: String) -> Result<Source, ProviderError> {
        let fd = fs.open(
            &path,
            SyncOpenOptions {
                read: true,
                ..SyncOpenOptions::default()
            },
        )?;
        let stat = fs.fstat(fd);
        let _ = fs.close(fd);
        let stat = stat?;
        if !stat.is_file {
            return Err(ProviderError::Other(format!(
                "{path:?} is not a regular file"
            )));
        }
        Ok(Source::File {
            fs,
            path: Arc::from(path),
            size: stat.size,
        })
    }

    pub(crate) fn size(&self) -> u64 {
        match self {
            Source::Bytes(bytes) => bytes.len() as u64,
            Source::File { size, .. } => *size,
        }
    }

    pub(crate) fn reader(&self) -> Result<SourceReader, Error> {
        match self {
            Source::Bytes(bytes) => Ok(SourceReader::Bytes(Cursor::new(bytes.clone()))),
            Source::File { fs, path, .. } => {
                Ok(SourceReader::File(FileReader::new(fs.clone(), path)?))
            }
        }
    }

    pub(crate) fn read_prefix(&self, len: usize) -> Result<Vec<u8>, Error> {
        let mut reader = self.reader()?;
        let mut buf = vec![0_u8; len];
        let mut read = 0;
        while read < len {
            let n = reader
                .read(&mut buf[read..])
                .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
            if n == 0 {
                break;
            }
            read += n;
        }
        buf.truncate(read);
        Ok(buf)
    }

    pub(crate) fn read_all(&self) -> Result<Vec<u8>, Error> {
        let mut reader = self.reader()?;
        let mut out = Vec::with_capacity(self.size().min(usize::MAX as u64) as usize);
        reader
            .read_to_end(&mut out)
            .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
        Ok(out)
    }
}

/// A `Read + Seek` view of one [`Source`]. File reads go through the jailed
/// [`SyncFileSystem`]; the fd is closed when the reader is dropped.
pub(crate) enum SourceReader {
    Bytes(Cursor<Arc<[u8]>>),
    File(FileReader),
}

impl Read for SourceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            SourceReader::Bytes(reader) => reader.read(buf),
            SourceReader::File(reader) => reader.read(buf),
        }
    }
}

impl Seek for SourceReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            SourceReader::Bytes(reader) => reader.seek(pos),
            SourceReader::File(reader) => reader.seek(pos),
        }
    }
}

pub(crate) struct FileReader {
    fs: Arc<dyn SyncFileSystem>,
    fd: SyncFd,
}

impl FileReader {
    fn new(fs: Arc<dyn SyncFileSystem>, path: &str) -> Result<FileReader, Error> {
        let fd = fs
            .open(
                path,
                SyncOpenOptions {
                    read: true,
                    ..SyncOpenOptions::default()
                },
            )
            .map_err(|error| Error::Decode(format!("could not open the archive: {error}")))?;
        Ok(FileReader { fs, fd })
    }
}

impl Read for FileReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self
            .fs
            .read(self.fd, buf)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(n)
    }
}

impl Seek for FileReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let (offset, whence) = match pos {
            SeekFrom::Start(offset) => {
                let offset = i64::try_from(offset)
                    .map_err(|_| std::io::Error::other("archive offset is too large"))?;
                (offset, SyncWhence::Start)
            }
            SeekFrom::Current(offset) => (offset, SyncWhence::Current),
            SeekFrom::End(offset) => (offset, SyncWhence::End),
        };
        let next = self
            .fs
            .seek(self.fd, offset, whence)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        Ok(next)
    }
}

impl Drop for FileReader {
    fn drop(&mut self) {
        let _ = self.fs.close(self.fd);
    }
}

/// Runs a future to completion on this thread. The stream workers use it to
/// block on a bounded channel send, which is what keeps a producer from racing
/// ahead of the consumer and holding chunks the extraction is not writing yet.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);

    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

fn send_event<T>(tx: &mut mpsc::Sender<T>, event: T) -> bool {
    block_on(tx.send(event)).is_ok()
}

/// Detects the format from its leading bytes. Names and extensions are never
/// consulted: an archive is what it starts with, not what it is called.
pub(crate) fn sniff(bytes: &[u8]) -> Format {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        Format::TarGz
    } else if bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
    {
        Format::Zip
    } else {
        Format::Tar
    }
}

/// Builds an archive deterministically. `gzip` is only meaningful for tar.
pub(crate) fn build(
    entries: Vec<InputEntry>,
    format: Format,
    compress: Compression,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    match (format, compress) {
        (Format::Tar | Format::TarGz, Compression::Gzip | Compression::None) => {}
        (Format::Zip, Compression::Deflate | Compression::None) => {}
        (format, compress) => {
            return Err(Error::Type(format!(
                "a {} archive cannot use {} compression",
                format.as_str(),
                compress.as_str()
            )));
        }
    }
    if entries.len() > limits.max_entries {
        return Err(too_large(format!(
            "the archive has {} entries; the limit is {}",
            entries.len(),
            limits.max_entries
        )));
    }
    let mut total: u64 = 0;
    let mut normalized = Vec::with_capacity(entries.len());
    for entry in entries {
        let (path, trailing) = validate_path(&entry.path)
            .map_err(|error| Error::Type(format!("entry {:?}: {error}", entry.path)))?;
        let directory = entry.data.is_none();
        if directory && !trailing {
            return Err(Error::Type(format!(
                "entry {:?}: a directory path must end with /",
                entry.path
            )));
        }
        if !directory && trailing {
            return Err(Error::Type(format!(
                "entry {:?}: a file path must not end with /",
                entry.path
            )));
        }
        let size = entry.data.as_ref().map_or(0, |data| data.len() as u64);
        total = total
            .checked_add(size)
            .ok_or_else(|| too_large("the archive's size overflowed"))?;
        if total > limits.max_bytes {
            return Err(too_large(format!(
                "the archive's uncompressed size exceeds {} bytes",
                limits.max_bytes
            )));
        }
        normalized.push(InputEntry {
            path,
            data: entry.data,
            modified_ms: entry.modified_ms,
        });
    }
    check_layout(
        &normalized
            .iter()
            .map(|entry| EntryMeta {
                path: entry.path.clone(),
                directory: entry.data.is_none(),
                size: entry.data.as_ref().map_or(0, |data| data.len() as u64),
                modified_ms: entry.modified_ms,
            })
            .collect::<Vec<_>>(),
    )?;
    normalized.sort_by(|a, b| a.path.cmp(&b.path));
    match format {
        Format::Tar | Format::TarGz => build_tar(
            &normalized,
            compress == Compression::Gzip || format == Format::TarGz,
        ),
        Format::Zip => build_zip(&normalized, compress == Compression::Deflate),
    }
}

fn build_tar(entries: &[InputEntry], gzip: bool) -> Result<Vec<u8>, Error> {
    let mut builder = tar::Builder::new(Vec::new());
    for entry in entries {
        let directory = entry.data.is_none();
        let data = entry.data.as_deref().unwrap_or(&[]);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(if directory {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        header.set_size(data.len() as u64);
        header.set_mode(if directory { 0o755 } else { 0o644 });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(entry.modified_ms.unwrap_or(0) / 1_000);
        let name = if directory {
            format!("{}/", entry.path)
        } else {
            entry.path.clone()
        };
        builder
            .append_data(&mut header, name, data)
            .map_err(|error| Error::Decode(format!("could not write tar entry: {error}")))?;
    }
    let bytes = builder
        .into_inner()
        .map_err(|error| Error::Decode(format!("could not finish the tar: {error}")))?;
    if !gzip {
        return Ok(bytes);
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&bytes)
        .and_then(|()| encoder.finish())
        .map_err(|error| Error::Decode(format!("could not gzip the tar: {error}")))
}

fn build_zip(entries: &[InputEntry], deflate: bool) -> Result<Vec<u8>, Error> {
    use zip::write::SimpleFileOptions;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let method = if deflate {
        zip::CompressionMethod::Deflated
    } else {
        zip::CompressionMethod::Stored
    };
    for entry in entries {
        let directory = entry.data.is_none();
        let data = entry.data.as_deref().unwrap_or(&[]);
        let options = SimpleFileOptions::default()
            .compression_method(method)
            .last_modified_time(zip_datetime(entry.modified_ms))
            .unix_permissions(if directory { 0o755 } else { 0o644 })
            .system(zip::System::Unix)
            .large_file(data.len() as u64 > u32::MAX as u64);
        if directory {
            writer
                .add_directory(format!("{}/", entry.path), options)
                .map_err(|error| Error::Decode(format!("could not write zip entry: {error}")))?;
        } else {
            writer
                .start_file(&entry.path, options)
                .map_err(|error| Error::Decode(format!("could not write zip entry: {error}")))?;
            writer
                .write_all(data)
                .map_err(|error| Error::Decode(format!("could not write zip entry: {error}")))?;
        }
    }
    writer
        .finish()
        .map(|cursor| cursor.into_inner())
        .map_err(|error| Error::Decode(format!("could not finish the zip: {error}")))
}

/// Reads an archive's metadata without materializing entry data.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn list(bytes: &[u8], limits: &Limits) -> Result<(Format, Vec<EntryMeta>), Error> {
    let format = sniff(bytes);
    let entries = match format {
        Format::Zip => read_zip(bytes, limits, false)?,
        Format::Tar => read_tar(Cursor::new(bytes), limits, false)?,
        Format::TarGz => {
            let tar = decompress(bytes, limits)?;
            read_tar(Cursor::new(tar), limits, false)?
        }
    };
    let mut metas = Vec::with_capacity(entries.len());
    for entry in entries {
        metas.push(entry.meta);
    }
    check_layout(&metas)?;
    Ok((format, metas))
}

/// Reads every entry, data included, bounded by `limits`.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn read_all(bytes: &[u8], limits: &Limits) -> Result<(Format, Vec<Entry>), Error> {
    let format = sniff(bytes);
    let entries = match format {
        Format::Zip => read_zip(bytes, limits, true)?,
        Format::Tar => read_tar(Cursor::new(bytes), limits, true)?,
        Format::TarGz => {
            let tar = decompress(bytes, limits)?;
            read_tar(Cursor::new(tar), limits, true)?
        }
    };
    check_layout(
        &entries
            .iter()
            .map(|entry| entry.meta.clone())
            .collect::<Vec<_>>(),
    )?;
    Ok((format, entries))
}

/// Lists a source's metadata without reading entry data. A file source is
/// scanned through a bounded reader, so only the metadata is retained.
pub(crate) fn list_source(
    source: &Source,
    limits: &Limits,
) -> Result<(Format, Vec<EntryMeta>), Error> {
    let head = source.read_prefix(8)?;
    let format = sniff(&head);
    let metas = match format {
        Format::Zip => zip_metas(source.reader()?, limits)?,
        Format::Tar => tar_metas(source.reader()?, limits)?,
        Format::TarGz => tar_metas(gzip_reader(source.reader()?, limits), limits)?,
    };
    check_layout(&metas)?;
    Ok((format, metas))
}

/// Reads every entry to its end and discards the bytes. Extraction runs this
/// before writing anything so a corrupt or truncated entry, including a bad
/// zip CRC, is reported while the destination is still untouched.
pub(crate) fn verify_source(source: &Source, format: Format, limits: &Limits) -> Result<(), Error> {
    match format {
        Format::Zip => verify_zip(source.reader()?, limits),
        Format::Tar => verify_tar(source.reader()?, limits),
        Format::TarGz => verify_tar(gzip_reader(source.reader()?, limits), limits),
    }
}

/// Reads only the named entries, in archive order. Unselected entries are
/// skipped rather than read into memory.
pub(crate) fn read_selected_source(
    source: &Source,
    format: Format,
    paths: &BTreeSet<String>,
    limits: &Limits,
) -> Result<Vec<Entry>, Error> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    match format {
        Format::Zip => zip_selected(source.reader()?, paths, limits),
        Format::Tar => tar_selected(source.reader()?, paths, limits),
        Format::TarGz => tar_selected(gzip_reader(source.reader()?, limits), paths, limits),
    }
}

/// Reads a whole source. Inherently allocates the archive, for `bytes()`.
pub(crate) fn source_bytes(source: &Source) -> Result<Vec<u8>, Error> {
    source.read_all()
}

/// Streams one entry's decompressed bytes from a worker thread. The bounded
/// channel is the backpressure: the worker cannot run further ahead than the
/// consumer allows.
pub(crate) fn spawn_entry_stream(
    source: Source,
    format: Format,
    path: String,
    limits: Limits,
    mut tx: mpsc::Sender<StreamEvent>,
) {
    let mut error_tx = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("archive-entry".to_string())
        .spawn(move || {
            let result = stream_entry(&source, format, &path, &limits, &mut tx);
            let event = match result {
                Ok(()) => StreamEvent::Done,
                Err(error) => StreamEvent::Error(error),
            };
            send_event(&mut tx, event);
        });
    if let Err(error) = spawned {
        send_event(
            &mut error_tx,
            StreamEvent::Error(Error::Decode(format!(
                "could not start the archive reader: {error}"
            ))),
        );
    }
}

/// Streams an archive's entries for extraction, one chunk at a time.
pub(crate) fn spawn_extract(
    source: Source,
    format: Format,
    limits: Limits,
    mut tx: mpsc::Sender<ExtractEvent>,
) {
    let mut error_tx = tx.clone();
    let spawned = std::thread::Builder::new()
        .name("archive-extract".to_string())
        .spawn(move || {
            let result = extract_entries(&source, format, &limits, &mut tx);
            let event = match result {
                Ok(()) => ExtractEvent::Done,
                Err(error) => ExtractEvent::Error(error),
            };
            send_event(&mut tx, event);
        });
    if let Err(error) = spawned {
        send_event(
            &mut error_tx,
            ExtractEvent::Error(Error::Decode(format!(
                "could not start the archive reader: {error}"
            ))),
        );
    }
}

fn gzip_reader(reader: SourceReader, limits: &Limits) -> impl Read {
    let cap = decompressed_cap(limits);
    flate2::read::GzDecoder::new(reader).take(cap)
}

fn decompressed_cap(limits: &Limits) -> u64 {
    (limits.max_entries as u64)
        .saturating_mul(1_024)
        .saturating_add(1 << 20)
        .saturating_add(limits.max_bytes)
        .saturating_add(1)
}

fn tar_metas<R: Read>(reader: R, limits: &Limits) -> Result<Vec<EntryMeta>, Error> {
    let mut archive = tar::Archive::new(reader);
    let mut metas = Vec::new();
    let mut total: u64 = 0;
    for item in archive.entries().map_err(tar_decode)? {
        let entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        check_entry_limits(&meta, metas.len(), &mut total, limits)?;
        metas.push(meta);
    }
    Ok(metas)
}

/// Reads the entry count from the zip's end record before the zip crate
/// builds its central-directory index. The index is allocated from this count,
/// so checking it afterwards would be after the memory had already been used.
/// `None` means the count could not be read cheaply; the post-parse check
/// still applies.
fn zip_precheck<R: Read + Seek>(reader: &mut R, limits: &Limits) -> Result<(), Error> {
    let declared = zip_declared_entries(reader)?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
    if let Some(count) = declared {
        check_zip_count(usize::try_from(count).unwrap_or(usize::MAX), limits)?;
    }
    Ok(())
}

fn check_zip_count(len: usize, limits: &Limits) -> Result<(), Error> {
    if len > limits.max_entries {
        return Err(too_large(format!(
            "the archive has {len} entries; the limit is {}",
            limits.max_entries
        )));
    }
    Ok(())
}

fn zip_declared_entries<R: Read + Seek>(reader: &mut R) -> Result<Option<u64>, Error> {
    let end = reader
        .seek(SeekFrom::End(0))
        .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
    let tail_len = end.min(65_557);
    reader
        .seek(SeekFrom::Start(end - tail_len))
        .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
    let mut tail = vec![0_u8; tail_len as usize];
    reader
        .read_exact(&mut tail)
        .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
    if tail.len() < 22 {
        return Ok(None);
    }
    let Some(eocd) = (0..=tail.len() - 22).rev().find(|&index| {
        tail[index..index + 4] == [0x50, 0x4b, 0x05, 0x06]
            && index + 22 + u16::from_le_bytes([tail[index + 20], tail[index + 21]]) as usize
                == tail.len()
    }) else {
        return Ok(None);
    };
    let total = u16::from_le_bytes([tail[eocd + 10], tail[eocd + 11]]);
    if total != u16::MAX {
        return Ok(Some(total as u64));
    }
    if eocd < 20 || tail[eocd - 20..eocd - 16] != [0x50, 0x4b, 0x06, 0x07] {
        return Ok(None);
    }
    let offset = u64::from_le_bytes(tail[eocd - 12..eocd - 4].try_into().unwrap());
    reader
        .seek(SeekFrom::Start(offset))
        .map_err(|error| Error::Decode(format!("could not read the archive: {error}")))?;
    let mut record = [0_u8; 56];
    if reader.read_exact(&mut record).is_err() || record[..4] != [0x50, 0x4b, 0x06, 0x06] {
        return Ok(None);
    }
    Ok(Some(u64::from_le_bytes(record[32..40].try_into().unwrap())))
}

fn zip_metas<R: Read + Seek>(mut reader: R, limits: &Limits) -> Result<Vec<EntryMeta>, Error> {
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let mut metas = Vec::with_capacity(archive.len());
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let meta = zip_meta(&mut archive, index)?;
        check_entry_limits(&meta, metas.len(), &mut total, limits)?;
        metas.push(meta);
    }
    Ok(metas)
}

fn check_entry_limits(
    meta: &EntryMeta,
    count: usize,
    total: &mut u64,
    limits: &Limits,
) -> Result<(), Error> {
    if count >= limits.max_entries {
        return Err(too_large(format!(
            "the archive has more than {} entries",
            limits.max_entries
        )));
    }
    if !meta.directory {
        *total = total
            .checked_add(meta.size)
            .ok_or_else(|| too_large("the archive's size overflowed"))?;
        if *total > limits.max_bytes {
            return Err(too_large(format!(
                "the archive's uncompressed size exceeds {} bytes",
                limits.max_bytes
            )));
        }
    }
    Ok(())
}

fn tar_selected<R: Read>(
    reader: R,
    paths: &BTreeSet<String>,
    limits: &Limits,
) -> Result<Vec<Entry>, Error> {
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for (count, item) in archive.entries().map_err(tar_decode)?.enumerate() {
        let mut entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        check_entry_limits(&meta, count, &mut total, limits)?;
        if paths.contains(&meta.path) {
            let data = if meta.directory {
                Vec::new()
            } else {
                read_tar_data(&mut entry, &meta)?
            };
            entries.push(Entry { meta, data });
        }
    }
    for path in paths {
        if !entries.iter().any(|entry| &entry.meta.path == path) {
            return Err(Error::NotFound(format!("no entry named {path:?}")));
        }
    }
    Ok(entries)
}

fn zip_selected<R: Read + Seek>(
    mut reader: R,
    paths: &BTreeSet<String>,
    limits: &Limits,
) -> Result<Vec<Entry>, Error> {
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let meta = zip_meta(&mut archive, index)?;
        check_entry_limits(&meta, index, &mut total, limits)?;
        if !paths.contains(&meta.path) {
            continue;
        }
        let data = if meta.directory {
            Vec::new()
        } else {
            let mut file = archive.by_index(index).map_err(zip_decode)?;
            let mut data = Vec::new();
            let mut limited = file.by_ref().take(meta.size.saturating_add(1));
            limited.read_to_end(&mut data).map_err(|error| {
                Error::Decode(format!("could not read {:?}: {error}", meta.path))
            })?;
            if data.len() as u64 != meta.size {
                return Err(Error::Decode(format!(
                    "entry {:?} does not match its declared size",
                    meta.path
                )));
            }
            data
        };
        entries.push(Entry { meta, data });
    }
    for path in paths {
        if !entries.iter().any(|entry| &entry.meta.path == path) {
            return Err(Error::NotFound(format!("no entry named {path:?}")));
        }
    }
    Ok(entries)
}

fn stream_entry(
    source: &Source,
    format: Format,
    path: &str,
    limits: &Limits,
    tx: &mut mpsc::Sender<StreamEvent>,
) -> Result<(), Error> {
    match format {
        Format::Zip => stream_zip(source.reader()?, path, limits, tx),
        Format::Tar => stream_tar(source.reader()?, path, limits, tx),
        Format::TarGz => stream_tar(gzip_reader(source.reader()?, limits), path, limits, tx),
    }
}

fn stream_tar<R: Read>(
    reader: R,
    path: &str,
    limits: &Limits,
    tx: &mut mpsc::Sender<StreamEvent>,
) -> Result<(), Error> {
    let mut archive = tar::Archive::new(reader);
    let mut total: u64 = 0;
    for (count, item) in archive.entries().map_err(tar_decode)?.enumerate() {
        let mut entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        check_entry_limits(&meta, count, &mut total, limits)?;
        if meta.path != path {
            continue;
        }
        if meta.directory {
            return Ok(());
        }
        return stream_reader(&mut entry, &meta, tx);
    }
    Err(Error::NotFound(format!("no entry named {path:?}")))
}

fn stream_zip<R: Read + Seek>(
    mut reader: R,
    path: &str,
    limits: &Limits,
    tx: &mut mpsc::Sender<StreamEvent>,
) -> Result<(), Error> {
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let index = archive
        .index_for_name(path)
        .ok_or_else(|| Error::NotFound(format!("no entry named {path:?}")))?;
    let meta = zip_meta(&mut archive, index)?;
    if meta.size > limits.max_bytes {
        return Err(too_large(format!(
            "entry {:?} exceeds {} bytes",
            meta.path, limits.max_bytes
        )));
    }
    if meta.directory {
        return Ok(());
    }
    let mut file = archive.by_index(index).map_err(zip_decode)?;
    stream_reader(&mut file, &meta, tx)
}

fn stream_reader<R: Read>(
    reader: &mut R,
    meta: &EntryMeta,
    tx: &mut mpsc::Sender<StreamEvent>,
) -> Result<(), Error> {
    let mut buf = vec![0_u8; STREAM_CHUNK];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|error| Error::Decode(format!("could not read {:?}: {error}", meta.path)))?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| too_large("the archive's size overflowed"))?;
        if total > meta.size {
            return Err(Error::Decode(format!(
                "entry {:?} is longer than its header says",
                meta.path
            )));
        }
        if !send_event(tx, StreamEvent::Data(buf[..n].to_vec())) {
            return Ok(());
        }
    }
    if total != meta.size {
        return Err(Error::Decode(format!(
            "entry {:?} is shorter than its header says",
            meta.path
        )));
    }
    Ok(())
}

fn extract_entries(
    source: &Source,
    format: Format,
    limits: &Limits,
    tx: &mut mpsc::Sender<ExtractEvent>,
) -> Result<(), Error> {
    match format {
        Format::Zip => extract_zip(source.reader()?, limits, tx),
        Format::Tar => extract_tar(source.reader()?, limits, tx),
        Format::TarGz => extract_tar(gzip_reader(source.reader()?, limits), limits, tx),
    }
}

fn extract_tar<R: Read>(
    reader: R,
    limits: &Limits,
    tx: &mut mpsc::Sender<ExtractEvent>,
) -> Result<(), Error> {
    let mut archive = tar::Archive::new(reader);
    let mut total: u64 = 0;
    for (count, item) in archive.entries().map_err(tar_decode)?.enumerate() {
        let mut entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        check_entry_limits(&meta, count, &mut total, limits)?;
        if !send_event(tx, ExtractEvent::Entry(meta.clone())) {
            return Ok(());
        }
        if !meta.directory {
            stream_extract_data(&mut entry, &meta, tx)?;
        }
        if !send_event(tx, ExtractEvent::EndEntry) {
            return Ok(());
        }
    }
    Ok(())
}

fn extract_zip<R: Read + Seek>(
    mut reader: R,
    limits: &Limits,
    tx: &mut mpsc::Sender<ExtractEvent>,
) -> Result<(), Error> {
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let meta = zip_meta(&mut archive, index)?;
        check_entry_limits(&meta, index, &mut total, limits)?;
        if !send_event(tx, ExtractEvent::Entry(meta.clone())) {
            return Ok(());
        }
        if !meta.directory {
            let mut file = archive.by_index(index).map_err(zip_decode)?;
            stream_extract_data(&mut file, &meta, tx)?;
        }
        if !send_event(tx, ExtractEvent::EndEntry) {
            return Ok(());
        }
    }
    Ok(())
}

fn verify_tar<R: Read>(reader: R, limits: &Limits) -> Result<(), Error> {
    let mut archive = tar::Archive::new(reader);
    let mut total: u64 = 0;
    for (count, item) in archive.entries().map_err(tar_decode)?.enumerate() {
        let mut entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        check_entry_limits(&meta, count, &mut total, limits)?;
        if !meta.directory {
            verify_reader(&mut entry, &meta)?;
        }
    }
    Ok(())
}

fn verify_zip<R: Read + Seek>(mut reader: R, limits: &Limits) -> Result<(), Error> {
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let mut total: u64 = 0;
    for index in 0..archive.len() {
        let meta = zip_meta(&mut archive, index)?;
        check_entry_limits(&meta, index, &mut total, limits)?;
        if !meta.directory {
            let mut file = archive.by_index(index).map_err(zip_decode)?;
            verify_reader(&mut file, &meta)?;
        }
    }
    Ok(())
}

/// Reads an entry to its declared end without keeping the bytes, so a bad
/// decompressor stream or a zip CRC mismatch is detected before a write.
fn verify_reader<R: Read>(reader: &mut R, meta: &EntryMeta) -> Result<(), Error> {
    let mut buf = vec![0_u8; STREAM_CHUNK];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|error| Error::Decode(format!("could not read {:?}: {error}", meta.path)))?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| too_large("the archive's size overflowed"))?;
        if total > meta.size {
            return Err(Error::Decode(format!(
                "entry {:?} is longer than its header says",
                meta.path
            )));
        }
    }
    if total != meta.size {
        return Err(Error::Decode(format!(
            "entry {:?} is shorter than its header says",
            meta.path
        )));
    }
    Ok(())
}

fn stream_extract_data<R: Read>(
    reader: &mut R,
    meta: &EntryMeta,
    tx: &mut mpsc::Sender<ExtractEvent>,
) -> Result<(), Error> {
    let mut buf = vec![0_u8; STREAM_CHUNK];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|error| Error::Decode(format!("could not read {:?}: {error}", meta.path)))?;
        if n == 0 {
            break;
        }
        total = total
            .checked_add(n as u64)
            .ok_or_else(|| too_large("the archive's size overflowed"))?;
        if total > meta.size {
            return Err(Error::Decode(format!(
                "entry {:?} is longer than its header says",
                meta.path
            )));
        }
        if !send_event(tx, ExtractEvent::Data(buf[..n].to_vec())) {
            return Ok(());
        }
    }
    if total != meta.size {
        return Err(Error::Decode(format!(
            "entry {:?} is shorter than its header says",
            meta.path
        )));
    }
    Ok(())
}

const STREAM_CHUNK: usize = 256 * 1024;

/// Reads one named entry, without materializing the others.
#[cfg(test)]
pub(crate) fn read_entry(
    bytes: &[u8],
    limits: &Limits,
    path: &str,
) -> Result<(Format, Entry), Error> {
    let format = sniff(bytes);
    match format {
        Format::Zip => {
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(zip_decode)?;
            let index = archive
                .index_for_name(path)
                .ok_or_else(|| Error::NotFound(format!("no entry named {path:?}")))?;
            let entry = zip_entry(&mut archive, index, limits)?;
            Ok((format, entry))
        }
        Format::Tar | Format::TarGz => {
            let reader: Box<dyn Read> = if format == Format::TarGz {
                Box::new(flate2::read::GzDecoder::new(Cursor::new(bytes)))
            } else {
                Box::new(Cursor::new(bytes))
            };
            let mut archive = tar::Archive::new(reader);
            let mut total: u64 = 0;
            let mut count: usize = 0;
            let mut named = None;
            for item in archive.entries().map_err(tar_decode)? {
                let mut entry = item.map_err(tar_decode)?;
                let meta = tar_meta(&entry)?;
                count += 1;
                if count > limits.max_entries {
                    return Err(too_large(format!(
                        "the archive has more than {} entries",
                        limits.max_entries
                    )));
                }
                if !meta.directory {
                    total = total
                        .checked_add(meta.size)
                        .ok_or_else(|| too_large("the archive's size overflowed"))?;
                    if total > limits.max_bytes {
                        return Err(too_large(format!(
                            "the archive's uncompressed size exceeds {} bytes",
                            limits.max_bytes
                        )));
                    }
                }
                if meta.path == path {
                    let data = read_tar_data(&mut entry, &meta)?;
                    named = Some(Entry { meta, data });
                    break;
                }
            }
            named
                .map(|entry| (format, entry))
                .ok_or_else(|| Error::NotFound(format!("no entry named {path:?}")))
        }
    }
}

#[cfg(any(test, feature = "fuzzing"))]
fn read_tar<R: Read>(reader: R, limits: &Limits, read_data: bool) -> Result<Vec<Entry>, Error> {
    let mut archive = tar::Archive::new(reader);
    let mut entries = Vec::new();
    let mut total: u64 = 0;
    for item in archive.entries().map_err(tar_decode)? {
        let mut entry = item.map_err(tar_decode)?;
        let meta = tar_meta(&entry)?;
        if entries.len() >= limits.max_entries {
            return Err(too_large(format!(
                "the archive has more than {} entries",
                limits.max_entries
            )));
        }
        if !meta.directory {
            total = total
                .checked_add(meta.size)
                .ok_or_else(|| too_large("the archive's size overflowed"))?;
            if total > limits.max_bytes {
                return Err(too_large(format!(
                    "the archive's uncompressed size exceeds {} bytes",
                    limits.max_bytes
                )));
            }
        }
        let data = if read_data && !meta.directory {
            read_tar_data(&mut entry, &meta)?
        } else {
            Vec::new()
        };
        entries.push(Entry { meta, data });
    }
    Ok(entries)
}

fn read_tar_data<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    meta: &EntryMeta,
) -> Result<Vec<u8>, Error> {
    let mut data = Vec::new();
    let mut limited = entry.take(meta.size.saturating_add(1));
    limited
        .read_to_end(&mut data)
        .map_err(|error| Error::Decode(format!("could not read {:?}: {error}", meta.path)))?;
    if data.len() as u64 != meta.size {
        return Err(Error::Decode(format!(
            "entry {:?} is shorter than its header says",
            meta.path
        )));
    }
    Ok(data)
}

fn tar_meta<R: Read>(entry: &tar::Entry<'_, R>) -> Result<EntryMeta, Error> {
    let raw = entry.path_bytes();
    let (path, trailing) = validate_path_bytes(&raw)?;
    let kind = entry.header().entry_type();
    let directory = match kind {
        tar::EntryType::Directory => true,
        tar::EntryType::Regular | tar::EntryType::Continuous => trailing,
        tar::EntryType::Symlink => {
            return Err(Error::UnsafePath(format!(
                "entry {path:?} is a symbolic link; links are not extracted"
            )));
        }
        tar::EntryType::Link => {
            return Err(Error::UnsafePath(format!(
                "entry {path:?} is a hard link; links are not extracted"
            )));
        }
        other => {
            return Err(Error::Unsupported(format!(
                "entry {path:?} has unsupported tar type {other:?}"
            )));
        }
    };
    let size = entry.size();
    if directory && size != 0 {
        return Err(Error::Decode(format!(
            "directory entry {path:?} carries data"
        )));
    }
    let modified_ms = entry
        .header()
        .mtime()
        .ok()
        .map(|seconds| seconds.saturating_mul(1_000));
    Ok(EntryMeta {
        path,
        directory,
        size,
        modified_ms,
    })
}

#[cfg(any(test, feature = "fuzzing"))]
fn read_zip(bytes: &[u8], limits: &Limits, read_data: bool) -> Result<Vec<Entry>, Error> {
    let mut reader = Cursor::new(bytes);
    zip_precheck(&mut reader, limits)?;
    let mut archive = zip::ZipArchive::new(reader).map_err(zip_decode)?;
    check_zip_count(archive.len(), limits)?;
    let mut total: u64 = 0;
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let meta = zip_meta(&mut archive, index)?;
        if !meta.directory {
            total = total
                .checked_add(meta.size)
                .ok_or_else(|| too_large("the archive's size overflowed"))?;
            if total > limits.max_bytes {
                return Err(too_large(format!(
                    "the archive's uncompressed size exceeds {} bytes",
                    limits.max_bytes
                )));
            }
        }
        let data = if read_data && !meta.directory {
            let mut file = archive.by_index(index).map_err(zip_decode)?;
            let mut data = Vec::new();
            let mut limited = file.by_ref().take(meta.size.saturating_add(1));
            limited.read_to_end(&mut data).map_err(|error| {
                Error::Decode(format!("could not read {:?}: {error}", meta.path))
            })?;
            if data.len() as u64 != meta.size {
                return Err(Error::Decode(format!(
                    "entry {:?} does not match its declared size",
                    meta.path
                )));
            }
            data
        } else {
            Vec::new()
        };
        entries.push(Entry { meta, data });
    }
    Ok(entries)
}

fn zip_meta<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
) -> Result<EntryMeta, Error> {
    let file = archive.by_index_raw(index).map_err(zip_decode)?;
    if file.encrypted() {
        return Err(Error::Encrypted(format!(
            "entry {:?} is encrypted",
            file.name()
        )));
    }
    let (path, _) = validate_path_bytes(file.name_raw())?;
    if file.is_symlink() {
        return Err(Error::UnsafePath(format!(
            "entry {path:?} is a symbolic link; links are not extracted"
        )));
    }
    let directory = file.is_dir();
    match file.compression() {
        zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated => {}
        other => {
            return Err(Error::Unsupported(format!(
                "entry {path:?} uses unsupported compression {other:?}"
            )));
        }
    }
    let size = file.size();
    if directory && size != 0 {
        return Err(Error::Decode(format!(
            "directory entry {path:?} carries data"
        )));
    }
    let modified_ms = file.last_modified().map(datetime_ms);
    Ok(EntryMeta {
        path,
        directory,
        size,
        modified_ms,
    })
}

#[cfg(test)]
fn zip_entry<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    limits: &Limits,
) -> Result<Entry, Error> {
    let meta = zip_meta(archive, index)?;
    if meta.size > limits.max_bytes {
        return Err(too_large(format!(
            "entry {:?} exceeds {} bytes",
            meta.path, limits.max_bytes
        )));
    }
    let data = if meta.directory {
        Vec::new()
    } else {
        let mut file = archive.by_index(index).map_err(zip_decode)?;
        let mut data = Vec::new();
        let mut limited = file.by_ref().take(meta.size.saturating_add(1));
        limited
            .read_to_end(&mut data)
            .map_err(|error| Error::Decode(format!("could not read {:?}: {error}", meta.path)))?;
        if data.len() as u64 != meta.size {
            return Err(Error::Decode(format!(
                "entry {:?} does not match its declared size",
                meta.path
            )));
        }
        data
    };
    Ok(Entry { meta, data })
}

/// Decompresses a gzip-wrapped tar, bounding the decompressed size so a bomb
/// cannot allocate more than the budget allows.
#[cfg(any(test, feature = "fuzzing"))]
fn decompress(bytes: &[u8], limits: &Limits) -> Result<Vec<u8>, Error> {
    let overhead = (limits.max_entries as u64)
        .saturating_mul(1_024)
        .saturating_add(1 << 20);
    let cap = limits.max_bytes.saturating_add(overhead);
    let mut decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
    let mut out = Vec::new();
    let mut limited = decoder.by_ref().take(cap.saturating_add(1));
    limited
        .read_to_end(&mut out)
        .map_err(|error| Error::Decode(format!("could not decompress the gzip stream: {error}")))?;
    if out.len() as u64 > cap {
        return Err(too_large(format!(
            "the decompressed tar exceeds {} bytes",
            cap
        )));
    }
    Ok(out)
}

/// Validates one path. Returns the normalized path (a directory entry's
/// trailing slash removed) and whether the original ended with `/`.
fn validate_path_bytes(raw: &[u8]) -> Result<(String, bool), Error> {
    let text = std::str::from_utf8(raw)
        .map_err(|_| Error::UnsafePath("an entry path is not valid UTF-8".to_string()))?;
    validate_path(text)
}

fn validate_path(raw: &str) -> Result<(String, bool), Error> {
    let trailing = raw.ends_with('/');
    let body = if trailing { &raw[..raw.len() - 1] } else { raw };
    if body.is_empty() {
        return Err(Error::UnsafePath("an entry path is empty".to_string()));
    }
    if body.contains('\0') {
        return Err(Error::UnsafePath(format!(
            "entry path {body:?} contains a NUL byte"
        )));
    }
    if body.contains('\\') {
        return Err(Error::UnsafePath(format!(
            "entry path {body:?} contains a backslash; use /"
        )));
    }
    if body.starts_with('/') {
        return Err(Error::UnsafePath(format!(
            "entry path {body:?} is absolute"
        )));
    }
    let bytes = body.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(Error::UnsafePath(format!(
            "entry path {body:?} starts with a drive letter"
        )));
    }
    for component in body.split('/') {
        if component.is_empty() {
            return Err(Error::UnsafePath(format!(
                "entry path {body:?} has an empty component"
            )));
        }
        if component == "." || component == ".." {
            return Err(Error::UnsafePath(format!(
                "entry path {body:?} contains a {component:?} component"
            )));
        }
    }
    Ok((body.to_string(), trailing))
}

/// Refuses duplicate paths and file/directory collisions. Names are compared
/// case-insensitively on Windows and macOS, where the filesystem would
/// otherwise merge two entries the archive treats as distinct.
fn check_layout(entries: &[EntryMeta]) -> Result<(), Error> {
    let fold = cfg!(any(windows, target_os = "macos"));
    let mut seen = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut files = BTreeSet::new();
    for entry in entries {
        if !seen.insert(entry.path.clone()) {
            return Err(Error::UnsafePath(format!(
                "duplicate entry path {:?}",
                entry.path
            )));
        }
        if fold && !folded.insert(entry.path.to_ascii_lowercase()) {
            return Err(Error::UnsafePath(format!(
                "entry path {:?} collides with another name on a case-insensitive filesystem",
                entry.path
            )));
        }
        if !entry.directory {
            files.insert(entry.path.as_str());
        }
    }
    for entry in entries {
        let mut end = entry.path.len();
        while let Some(position) = entry.path[..end].rfind('/') {
            let ancestor = &entry.path[..position];
            if files.contains(ancestor) {
                return Err(Error::UnsafePath(format!(
                    "entry {:?} is nested under the file {ancestor:?}",
                    entry.path
                )));
            }
            end = position;
        }
    }
    Ok(())
}

fn too_large(message: impl Into<String>) -> Error {
    Error::TooLarge(message.into())
}

fn tar_decode(error: std::io::Error) -> Error {
    Error::Decode(format!("invalid tar: {error}"))
}

fn zip_decode(error: zip::result::ZipError) -> Error {
    match error {
        zip::result::ZipError::UnsupportedArchive(message)
            if message == zip::result::ZipError::PASSWORD_REQUIRED =>
        {
            Error::Encrypted("the zip archive is encrypted".to_string())
        }
        zip::result::ZipError::InvalidPassword => {
            Error::Encrypted("the zip archive is encrypted".to_string())
        }
        zip::result::ZipError::UnsupportedArchive(message) => {
            Error::Unsupported(format!("unsupported zip feature: {message}"))
        }
        zip::result::ZipError::CompressionMethodNotSupported(method) => {
            Error::Unsupported(format!("unsupported zip compression method {method}"))
        }
        zip::result::ZipError::FileNotFound => {
            Error::Decode("the zip archive names an entry it does not contain".to_string())
        }
        other => Error::Decode(format!("invalid zip: {other}")),
    }
}

// ---- time ------------------------------------------------------------------
//
// Zip stores civil time (no zone) at two-second resolution. We read it as UTC,
// which is what makes a listing deterministic, and write UTC the same way.

fn datetime_ms(datetime: zip::DateTime) -> u64 {
    let days = days_from_civil(
        i64::from(datetime.year()),
        i64::from(datetime.month()),
        i64::from(datetime.day()),
    );
    let seconds = days * 86_400
        + i64::from(datetime.hour()) * 3_600
        + i64::from(datetime.minute()) * 60
        + i64::from(datetime.second());
    seconds.max(0) as u64 * 1_000
}

fn zip_datetime(ms: Option<u64>) -> zip::DateTime {
    let Some(ms) = ms else {
        return zip::DateTime::default();
    };
    let seconds = (ms / 1_000) as i64;
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(1980..=2107).contains(&year) {
        return zip::DateTime::default();
    }
    zip::DateTime::from_date_and_time(
        year as u16,
        month as u8,
        day as u8,
        (of_day / 3_600) as u8,
        ((of_day % 3_600) / 60) as u8,
        (of_day % 60) as u8,
    )
    .unwrap_or_default()
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, data: &[u8]) -> InputEntry {
        InputEntry {
            path: path.to_string(),
            data: Some(data.to_vec()),
            modified_ms: None,
        }
    }

    fn dir(path: &str) -> InputEntry {
        InputEntry {
            path: path.to_string(),
            data: None,
            modified_ms: None,
        }
    }

    fn sample() -> Vec<InputEntry> {
        vec![
            file("b.txt", b"second"),
            file("a/c.txt", b"nested"),
            file("a.txt", b"first"),
            dir("empty/"),
        ]
    }

    #[test]
    fn tar_round_trip_is_deterministic() {
        let first = build(sample(), Format::Tar, Compression::None, &Limits::default()).unwrap();
        let second = build(sample(), Format::Tar, Compression::None, &Limits::default()).unwrap();
        assert_eq!(first, second);
        let (format, entries) = read_all(&first, &Limits::default()).unwrap();
        assert_eq!(format, Format::Tar);
        let names: Vec<_> = entries
            .iter()
            .map(|entry| entry.meta.path.as_str())
            .collect();
        assert_eq!(names, ["a.txt", "a/c.txt", "b.txt", "empty"]);
        assert_eq!(entries[0].data, b"first");
        assert!(entries[3].meta.directory);
    }

    #[test]
    fn tar_gz_round_trip() {
        let bytes = build(
            sample(),
            Format::TarGz,
            Compression::Gzip,
            &Limits::default(),
        )
        .unwrap();
        assert_eq!(sniff(&bytes), Format::TarGz);
        let (format, entries) = read_all(&bytes, &Limits::default()).unwrap();
        assert_eq!(format, Format::TarGz);
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn zip_round_trip() {
        let bytes = build(sample(), Format::Zip, Compression::None, &Limits::default()).unwrap();
        assert_eq!(sniff(&bytes), Format::Zip);
        let (format, entries) = read_all(&bytes, &Limits::default()).unwrap();
        assert_eq!(format, Format::Zip);
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.meta.path == "a/c.txt")
                .unwrap()
                .data,
            b"nested"
        );
    }

    #[test]
    fn read_entry_finds_one_name() {
        for format in [Format::Tar, Format::TarGz, Format::Zip] {
            let gzip = format == Format::TarGz;
            let compress = if gzip {
                Compression::Gzip
            } else {
                Compression::None
            };
            let bytes = build(sample(), format, compress, &Limits::default()).unwrap();
            let (_, entry) = read_entry(&bytes, &Limits::default(), "a/c.txt").unwrap();
            assert_eq!(entry.data, b"nested");
            assert_eq!(
                read_entry(&bytes, &Limits::default(), "missing").unwrap_err(),
                Error::NotFound("no entry named \"missing\"".to_string())
            );
        }
    }

    #[test]
    fn zip_entry_count_is_read_before_the_central_directory() {
        let bytes = build(sample(), Format::Zip, Compression::None, &Limits::default()).unwrap();
        let mut cursor = Cursor::new(bytes.as_slice());
        assert_eq!(zip_declared_entries(&mut cursor).unwrap(), Some(4));
        let limits = Limits {
            max_entries: 2,
            max_bytes: 1_000,
        };
        assert!(matches!(list(&bytes, &limits), Err(Error::TooLarge(_))));

        // Corrupt the central directory but leave the end record's count
        // intact: the limit must fire before the parser reads that directory.
        let eocd = bytes
            .windows(4)
            .rposition(|window| window == [0x50, 0x4b, 0x05, 0x06])
            .unwrap();
        let directory =
            u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
        let mut broken = bytes.clone();
        broken[directory] ^= 0xff;
        assert!(matches!(list(&broken, &limits), Err(Error::TooLarge(_))));
    }

    #[test]
    fn limits_are_enforced() {
        let limits = Limits {
            max_entries: 2,
            max_bytes: 1_000,
        };
        assert!(matches!(
            build(sample(), Format::Tar, Compression::None, &limits),
            Err(Error::TooLarge(_))
        ));
        let bytes = build(sample(), Format::Tar, Compression::None, &Limits::default()).unwrap();
        assert!(matches!(list(&bytes, &limits), Err(Error::TooLarge(_))));
    }

    #[test]
    fn unsafe_paths_are_refused() {
        for path in [
            "",
            "/etc/passwd",
            "../escape",
            "a/../../b",
            "C:/windows",
            "a\\b",
            "a//b",
            "./a",
        ] {
            let result = build(
                vec![file(path, b"x")],
                Format::Tar,
                Compression::None,
                &Limits::default(),
            );
            assert!(result.is_err(), "{path:?} was accepted");
        }
    }

    #[test]
    fn duplicates_and_collisions_are_refused() {
        let duplicate = vec![file("a.txt", b"1"), file("a.txt", b"2")];
        assert!(matches!(
            build(
                duplicate,
                Format::Tar,
                Compression::None,
                &Limits::default()
            ),
            Err(Error::UnsafePath(_))
        ));
        let collision = vec![file("a", b"1"), file("a/b", b"2")];
        assert!(matches!(
            build(
                collision,
                Format::Tar,
                Compression::None,
                &Limits::default()
            ),
            Err(Error::UnsafePath(_))
        ));
    }

    /// A tar with one entry, written by hand: the builder refuses the names the
    /// reader must still refuse. `tar` is a byte layout, so a test can write
    /// the header the builder would not.
    fn raw_tar(name: &str, data: &[u8]) -> Vec<u8> {
        let mut header = [0_u8; 512];
        assert!(name.len() <= 100);
        header[..name.len()].copy_from_slice(name.as_bytes());
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0000000");
        header[116..123].copy_from_slice(b"0000000");
        header[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].fill(b' ');
        let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
        header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut bytes = header.to_vec();
        bytes.extend_from_slice(data);
        bytes.resize(512 + data.len().div_ceil(512) * 512, 0);
        bytes.extend_from_slice(&[0_u8; 1024]);
        bytes
    }

    #[test]
    fn foreign_archives_are_validated_on_read() {
        for name in [
            "../escape",
            "/etc/passwd",
            "a/../../b",
            "C:/windows",
            "a\\b",
        ] {
            let bytes = raw_tar(name, b"x");
            assert!(
                matches!(list(&bytes, &Limits::default()), Err(Error::UnsafePath(_))),
                "{name:?} was accepted"
            );
        }
        let clean = raw_tar("fine.txt", b"x");
        let (_, entries) = read_all(&clean, &Limits::default()).unwrap();
        assert_eq!(entries[0].meta.path, "fine.txt");
    }

    #[test]
    fn zip_links_are_refused() {
        use zip::write::SimpleFileOptions;
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .add_symlink("link", "../target", SimpleFileOptions::default())
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(matches!(
            list(&bytes, &Limits::default()),
            Err(Error::UnsafePath(_))
        ));
    }

    #[test]
    fn civil_dates_round_trip() {
        for days in [-20_000, -1, 0, 1, 10_957, 46_000] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    }
}
