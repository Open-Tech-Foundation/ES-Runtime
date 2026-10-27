//! Host ops backing `runtime:pdf` (DECISIONS.md D190), on otf-pdf.
//!
//! A document is opened once and read through a handle: the parsed
//! [`Document`] sits behind an `Arc` in this agent's registry, so `pages`,
//! `text` and the rest reuse what the first read parsed. The registry is
//! built inside `install`, once per runtime, so a handle names a document of
//! this agent and of no other (D50).
//!
//! Edits never touch that document. `pdf_save` gets the handle and the list
//! of steps `pdf.js` recorded, re-opens the handle's bytes (shared, not
//! copied) with the options it was opened with, applies the steps and saves.
//!
//! Cancellation is otf-pdf's [`Budget`]: a token is a flag the guest's
//! `AbortSignal` sets through `pdf_cancel`, and the budget refuses further
//! work once it is set. A document opened with a token keeps it for every
//! later read.
//!
//! The work runs on the embedder's [`TaskSpawner`] (otf-pdf is synchronous
//! and has no pool), inline without one. A PDF failure resolves as
//! `{ error, message }` rather than rejecting, so `pdf.js` maps it to the
//! module's `ERR_PDF_*` codes in one place; filesystem and entropy failures
//! reject as their own modules' do.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{Entropy, FileSystem, ProviderError, TaskSpawner};
use otf_pdf::{
    AttachmentOptions, Budget, Document, Encryption, Error as PdfError, Info, LimitKind, Limits,
    Object, OpenOptions, OutlineItem, Page, Permissions, Rect, SanitizeOptions, SaveMode,
    SaveOptions, Source, SourceError, Strictness, TextExtractor, Version,
};

use crate::Result;

/// One stream's decoded size, for a runtime that serves uploads. otf-pdf's
/// default is 512 MiB; a single stream is what a small upload inflates.
const MAX_DECODED_SIZE: u64 = 64 * 1024 * 1024;

/// The bytes a document was opened from, shared by the open document and
/// every save that re-opens it.
#[derive(Clone)]
struct Shared(Arc<Vec<u8>>);

impl Source for Shared {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::result::Result<usize, SourceError> {
        self.0.as_slice().read_at(offset, buf)
    }
}

/// A cancellation flag as otf-pdf's work budget.
struct Cancel(Arc<AtomicBool>);

impl Budget for Cancel {
    fn charge(&self, _units: u64) -> bool {
        !self.0.load(Ordering::Relaxed)
    }
}

/// otf-pdf's `Entropy` cannot fail; the provider's can. A failed draw is
/// remembered and the save that asked for it is discarded, so no file is
/// encrypted with anything but the provider's bytes.
struct Drawn {
    entropy: Arc<dyn Entropy>,
    failed: std::sync::Mutex<Option<ProviderError>>,
}

impl otf_pdf::Entropy for Drawn {
    fn fill(&self, buf: &mut [u8]) {
        if let Err(e) = self.entropy.fill(buf) {
            buf.fill(0);
            let mut failed = self.failed.lock().unwrap_or_else(|p| p.into_inner());
            failed.get_or_insert(e);
        }
    }
}

struct PdfHandle {
    bytes: Shared,
    /// What it was opened with, minus the budget: a save re-opens with these
    /// and its own token.
    password: Vec<u8>,
    strictness: Strictness,
    max_decoded_size: u64,
    doc: Arc<Document>,
}

type Handles = Rc<RefCell<HashMap<u64, Rc<PdfHandle>>>>;
type Tokens = Rc<RefCell<HashMap<u64, Arc<AtomicBool>>>>;

/// How a document is opened, as `pdf.js` passes it.
struct Open {
    password: Vec<u8>,
    strictness: Strictness,
    max_decoded_size: u64,
}

impl Open {
    fn options(&self, cancel: Option<Arc<AtomicBool>>) -> OpenOptions {
        let limits = Limits {
            max_decoded_size: self.max_decoded_size,
            budget: cancel.map(|flag| Arc::new(Cancel(flag)) as Arc<dyn Budget>),
            ..Limits::default()
        };
        OpenOptions {
            password: self.password.clone(),
            strictness: self.strictness,
            limits,
        }
    }
}

/// One edit, as `pdf.js` records it.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Select(Vec<usize>),
    Delete(Vec<usize>),
    Rotate(Vec<usize>, i32),
    Crop(Vec<usize>, Rect),
    MediaBox(Vec<usize>, Rect),
    Append(Box<Nested>, Option<Vec<usize>>),
    SetInfo(Vec<(String, Option<String>)>),
    AddAttachment {
        name: String,
        data: Vec<u8>,
        options: AttachmentOptions,
    },
    RemoveAttachment(String),
    Sanitize(SanitizeOptions),
    SetOutlines(Vec<Bookmark>),
}

/// A bookmark as `setOutlines` takes it; `page` is resolved against the
/// pages the document has when the step runs.
#[derive(Debug, Clone, PartialEq)]
struct Bookmark {
    title: String,
    page: Option<usize>,
    open: bool,
    children: Vec<Bookmark>,
}

/// Another document and the edits made to it, for `append`.
#[derive(Debug, Clone, PartialEq)]
struct Nested {
    handle: u64,
    steps: Vec<Step>,
}

/// A step whose `append` names a resolved document. Everything in it is
/// `Send`, so it travels to the spawner.
enum ResolvedStep {
    Plain(Step),
    Append(Box<Job>, Option<Vec<usize>>),
}

/// What a save needs once it leaves the isolate thread.
struct Job {
    bytes: Shared,
    open: Open,
    doc: Arc<Document>,
    steps: Vec<ResolvedStep>,
}

/// Registers the `runtime:pdf` ops.
pub(crate) fn install(
    engine: &mut dyn Engine,
    fs: Option<Arc<dyn FileSystem>>,
    entropy: Arc<dyn Entropy>,
    spawner: Option<Arc<dyn TaskSpawner>>,
) -> Result<()> {
    let handles: Handles = Rc::new(RefCell::new(HashMap::new()));
    let tokens: Tokens = Rc::new(RefCell::new(HashMap::new()));
    let next = Rc::new(RefCell::new(0_u64));

    let (ids, toks) = (next.clone(), tokens.clone());
    engine.register_op(OpDecl::sync("pdf_cancel_token", move |_args| {
        let id = mint(&ids);
        toks.borrow_mut()
            .insert(id, Arc::new(AtomicBool::new(false)));
        Ok(Value::Number(id as f64))
    }))?;

    let toks = tokens.clone();
    engine.register_op(OpDecl::sync("pdf_cancel", move |args| {
        let id = number_arg(&args, 0, "token")?;
        if let Some(flag) = toks.borrow().get(&id) {
            flag.store(true, Ordering::Relaxed);
        }
        Ok(Value::Undefined)
    }))?;

    let toks = tokens.clone();
    engine.register_op(OpDecl::sync("pdf_cancel_release", move |args| {
        let id = number_arg(&args, 0, "token")?;
        toks.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    for (name, from_file) in [("pdf_open", false), ("pdf_open_file", true)] {
        let (reg, ids, toks, fs, spawner) = (
            handles.clone(),
            next.clone(),
            tokens.clone(),
            fs.clone(),
            spawner.clone(),
        );
        let mut decl = OpDecl::r#async(name, move |mut args| {
            let (reg, ids, fs, spawner) = (reg.clone(), ids.clone(), fs.clone(), spawner.clone());
            let parsed = (|| {
                let source = if from_file {
                    Input::File(string_arg(&args, 0, "source path")?)
                } else {
                    Input::Bytes(take_bytes(&mut args, 0, "source")?)
                };
                let open = parse_open(args.get(1))?;
                let cancel = token_arg(&toks, args.get(2))?;
                Ok::<_, OpError>((source, open, cancel))
            })();
            Box::pin(async move {
                let (source, open, cancel) = parsed?;
                let bytes = match source {
                    Input::Bytes(bytes) => bytes,
                    Input::File(path) => require(&fs)?.read(path).await.map_err(map_err)?,
                };
                let bytes = Shared(Arc::new(bytes));
                let source = bytes.clone();
                let options = open.options(cancel.clone());
                let opened = offload(spawner, move || {
                    let doc = Document::open(source, options)?;
                    let summary = summary(&doc)?;
                    Ok::<_, PdfError>((doc, summary))
                })
                .await?;
                let (doc, mut summary) = match opened {
                    Ok(opened) => opened,
                    Err(error) => return Ok(failure(&error, cancel.as_deref())),
                };
                let id = mint(&ids);
                reg.borrow_mut().insert(
                    id,
                    Rc::new(PdfHandle {
                        bytes,
                        password: open.password,
                        strictness: open.strictness,
                        max_decoded_size: open.max_decoded_size,
                        doc: Arc::new(doc),
                    }),
                );
                summary.insert(0, field("handle", Value::Number(id as f64)));
                Ok(ok(Value::Object(summary)))
            })
        });
        if from_file {
            decl = decl.requires(Capability::FileRead).target_arg(0);
        }
        engine.register_op(decl)?;
    }

    // The reads: each takes a handle, and some an argument, and runs on the
    // spawner against the shared document.
    type Read = fn(&Document, &[Value]) -> std::result::Result<Value, PdfError>;
    for (name, read) in [
        ("pdf_pages", pages as Read),
        ("pdf_text", text),
        ("pdf_page_text", page_text),
        ("pdf_outlines", outlines),
        ("pdf_attachments", attachments),
        ("pdf_attachment", attachment),
        ("pdf_xmp", xmp),
        ("pdf_destinations", destinations),
    ] {
        let (reg, spawner) = (handles.clone(), spawner.clone());
        engine.register_op(OpDecl::r#async(name, move |mut args| {
            let spawner = spawner.clone();
            let parsed = lookup(&reg, &args).map(|handle| {
                let rest: Vec<Value> = args.drain(1..).collect();
                (handle.doc.clone(), rest)
            });
            Box::pin(async move {
                let (doc, rest) = parsed?;
                let result = offload(spawner, move || read(&doc, &rest)).await?;
                Ok(match result {
                    Ok(value) => ok(value),
                    Err(error) => failure(&error, None),
                })
            })
        }))?;
    }

    // What `sanitize(options)` would remove. Sanitizing needs a document of
    // its own to change, so the bytes are opened again and nothing is kept.
    let (reg, s) = (handles.clone(), spawner.clone());
    engine.register_op(OpDecl::r#async("pdf_sanitize_report", move |args| {
        let spawner = s.clone();
        let parsed = lookup(&reg, &args).map(|handle| {
            let options = sanitize_options(args.get(1));
            (handle.bytes.clone(), handle_open(&handle), options)
        });
        Box::pin(async move {
            let (bytes, open, options) = parsed?;
            let result = offload(spawner, move || {
                let mut doc = Document::open(bytes, open.options(None))?;
                doc.sanitize(&options)
            })
            .await?;
            Ok(match result {
                Ok(report) => ok(Value::Object(vec![
                    field("javascript", num(report.javascript as f64)),
                    field("externalActions", num(report.external_actions as f64)),
                    field("links", num(report.links as f64)),
                    field("attachments", num(report.attachments as f64)),
                    field("xfa", num(report.xfa as f64)),
                    field("multimedia", num(report.multimedia as f64)),
                    field("metadata", num(report.metadata as f64)),
                ])),
                Err(error) => failure(&error, None),
            })
        })
    }))?;

    let reg = handles.clone();
    engine.register_op(OpDecl::sync("pdf_close", move |args| {
        let id = number_arg(&args, 0, "handle")?;
        reg.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    for (name, to_file) in [("pdf_save", false), ("pdf_save_file", true)] {
        let (reg, toks, fs, entropy, spawner) = (
            handles.clone(),
            tokens.clone(),
            fs.clone(),
            entropy.clone(),
            spawner.clone(),
        );
        let mut decl = OpDecl::r#async(name, move |args| {
            let (fs, entropy, spawner) = (fs.clone(), entropy.clone(), spawner.clone());
            let parsed = (|| {
                let handle = lookup(&reg, &args)?;
                let steps = parse_steps(args.get(1))?;
                let job = resolve(&reg, handle, steps)?;
                let save = parse_save(args.get(2))?;
                let cancel = token_arg(&toks, args.get(3))?;
                let dest = if to_file {
                    Some(string_arg(&args, 4, "destination path")?)
                } else {
                    None
                };
                Ok::<_, OpError>((job, save, cancel, dest))
            })();
            Box::pin(async move {
                let (job, save, cancel, dest) = parsed?;
                let drawn = Arc::new(Drawn {
                    entropy,
                    failed: std::sync::Mutex::new(None),
                });
                let flag = cancel.clone();
                let rng = drawn.clone();
                let saved = offload(spawner, move || run_save(job, save, flag, rng)).await?;
                if let Some(e) = drawn
                    .failed
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .take()
                {
                    return Err(map_err(e));
                }
                let bytes = match saved {
                    Ok(bytes) => bytes,
                    Err(error) => return Ok(failure(&error, cancel.as_deref())),
                };
                Ok(ok(match dest {
                    Some(path) => {
                        let written = require(&fs)?
                            .write(path, bytes, false)
                            .await
                            .map_err(map_err)?;
                        Value::Number(written as f64)
                    }
                    None => Value::Bytes(bytes),
                }))
            })
        });
        if to_file {
            decl = decl.requires(Capability::FileWrite).target_arg(4);
        }
        engine.register_op(decl)?;
    }
    Ok(())
}

enum Input {
    Bytes(Vec<u8>),
    File(String),
}

/// Runs `work` on the spawner, or inline without one.
async fn offload<T: Send + 'static>(
    spawner: Option<Arc<dyn TaskSpawner>>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, OpError> {
    let Some(spawner) = spawner else {
        return Ok(work());
    };
    let (tx, rx) = futures_channel::oneshot::channel();
    spawner
        .spawn_blocking(Box::new(move || {
            let _ = tx.send(work());
        }))
        .await;
    // A dropped sender means the work never finished: the pool shut down, or
    // the work panicked, which otf-pdf promises malformed input cannot cause.
    rx.await
        .map_err(|_| OpError::new(ExceptionClass::Error, "the PDF work did not complete"))
}

// ---- saving -----------------------------------------------------------------

/// Resolves every handle a save names while still on the isolate thread, so
/// a closed or foreign document is refused at the call.
fn resolve(
    reg: &Handles,
    handle: Rc<PdfHandle>,
    steps: Vec<Step>,
) -> std::result::Result<Job, OpError> {
    let mut out = Vec::with_capacity(steps.len());
    for step in steps {
        out.push(match step {
            Step::Append(nested, pages) => {
                let other = reg
                    .borrow()
                    .get(&nested.handle)
                    .cloned()
                    .ok_or_else(closed)?;
                ResolvedStep::Append(Box::new(resolve(reg, other, nested.steps)?), pages)
            }
            step => ResolvedStep::Plain(step),
        });
    }
    Ok(Job {
        bytes: handle.bytes.clone(),
        open: handle_open(&handle),
        doc: handle.doc.clone(),
        steps: out,
    })
}

/// The options `handle` was opened with, to open its bytes again.
fn handle_open(handle: &PdfHandle) -> Open {
    Open {
        password: handle.password.clone(),
        strictness: handle.strictness,
        max_decoded_size: handle.max_decoded_size,
    }
}

/// The document a job describes: the open one when there is nothing to
/// change, else a fresh open of the same bytes with the steps applied.
fn build(
    job: Job,
    cancel: &Option<Arc<AtomicBool>>,
) -> std::result::Result<Arc<Document>, PdfError> {
    if job.steps.is_empty() {
        return Ok(job.doc);
    }
    let mut doc = Document::open(job.bytes, job.open.options(cancel.clone()))?;
    for step in job.steps {
        match step {
            ResolvedStep::Append(other, pages) => {
                let other = build(*other, cancel)?;
                doc.append_pages_from(&other, pages.as_deref())?;
            }
            ResolvedStep::Plain(step) => apply(&mut doc, step)?,
        }
    }
    Ok(Arc::new(doc))
}

fn apply(doc: &mut Document, step: Step) -> std::result::Result<(), PdfError> {
    match step {
        Step::Select(pages) => doc.select_pages(&pages),
        Step::Delete(pages) => doc.delete_pages(&pages),
        Step::Rotate(pages, degrees) => pages
            .into_iter()
            .try_for_each(|page| doc.rotate_page(page, degrees)),
        Step::Crop(pages, rect) => pages
            .into_iter()
            .try_for_each(|page| doc.set_crop_box(page, rect)),
        Step::MediaBox(pages, rect) => pages
            .into_iter()
            .try_for_each(|page| doc.set_media_box(page, rect)),
        Step::SetOutlines(bookmarks) => {
            let pages: Vec<_> = doc.pages()?.iter().map(|p| p.id).collect();
            let items: Vec<OutlineItem> = bookmarks
                .iter()
                .map(|b| outline_item(b, &pages))
                .collect::<std::result::Result<_, _>>()?;
            doc.set_outlines(&items)
        }
        Step::SetInfo(fields) => {
            let mut info = doc.info();
            for (key, value) in fields {
                let slot = match key.as_str() {
                    "title" => &mut info.title,
                    "author" => &mut info.author,
                    "subject" => &mut info.subject,
                    "keywords" => &mut info.keywords,
                    "creator" => &mut info.creator,
                    "producer" => &mut info.producer,
                    "creationDate" => &mut info.creation_date,
                    "modDate" => &mut info.mod_date,
                    _ => continue,
                };
                *slot = value;
            }
            doc.set_info(&info)
        }
        Step::AddAttachment {
            name,
            data,
            options,
        } => doc.add_attachment(&name, &data, &options),
        Step::RemoveAttachment(name) => doc.remove_attachment(&name).map(drop),
        Step::Sanitize(options) => doc.sanitize(&options).map(drop),
        Step::Append(..) => unreachable!("an append is resolved before the work runs"),
    }
}

/// The save options `pdf.js` passes. The encryption's entropy is filled in
/// by [`run_save`].
#[derive(Debug, Clone, PartialEq)]
struct Save {
    incremental: bool,
    version: Version,
    object_streams: bool,
    compress: bool,
    encryption: Option<Encryption>,
}

impl Default for Save {
    fn default() -> Self {
        let defaults = SaveOptions::default();
        Self {
            incremental: false,
            version: defaults.version,
            object_streams: defaults.object_streams,
            compress: defaults.compress,
            encryption: None,
        }
    }
}

fn run_save(
    job: Job,
    save: Save,
    cancel: Option<Arc<AtomicBool>>,
    entropy: Arc<Drawn>,
) -> std::result::Result<Vec<u8>, PdfError> {
    let doc = build(job, &cancel)?;
    let options = SaveOptions {
        mode: if save.incremental {
            SaveMode::Incremental
        } else {
            SaveMode::Full
        },
        version: save.version,
        object_streams: save.object_streams,
        compress: save.compress,
        entropy: save
            .encryption
            .is_some()
            .then_some(entropy as Arc<dyn otf_pdf::Entropy>),
        encryption: save.encryption,
        ..SaveOptions::default()
    };
    doc.save(&options)
        .map_err(|error| refused(error, save.incremental))
}

/// otf-pdf refuses an unsafe incremental save with an ordinary error whose
/// message ends "save in full"; it becomes its own failure here so the guest
/// can fall back without parsing the message.
fn refused(error: PdfError, incremental: bool) -> PdfError {
    error_refusal(&error, incremental).unwrap_or(error)
}

fn error_refusal(error: &PdfError, incremental: bool) -> Option<PdfError> {
    if !incremental {
        return None;
    }
    let message = match error {
        PdfError::Syntax { message, .. } => (*message).to_string(),
        PdfError::UnsupportedEncryption(message) => message.clone(),
        _ => return None,
    };
    message
        .ends_with("save in full")
        .then(|| PdfError::UnsupportedEncryption(format!("{REFUSED}{message}")))
}

/// Marks an incremental refusal so [`failure`] can name it.
const REFUSED: &str = "\u{0}refused:";

// ---- reads ------------------------------------------------------------------

fn summary(doc: &Document) -> std::result::Result<Vec<(String, Value)>, PdfError> {
    let mut out = vec![
        field("pageCount", num(doc.page_count()? as f64)),
        field(
            "version",
            doc.version().map_or(Value::Null, |v| {
                Value::String(format!("{}.{}", v.major, v.minor))
            }),
        ),
        field("encrypted", Value::Bool(doc.is_encrypted())),
        field("info", info_value(&doc.info())),
        field(
            "diagnostics",
            Value::Array(
                doc.diagnostics()
                    .iter()
                    .map(|d| {
                        let mut fields = vec![
                            field("kind", Value::String(d.kind.to_string())),
                            field("message", Value::String(d.to_string())),
                        ];
                        if let Some(offset) = d.offset {
                            fields.push(field("offset", num(offset as f64)));
                        }
                        Value::Object(fields)
                    })
                    .collect(),
            ),
        ),
    ];
    let permissions = doc
        .file()
        .security()
        .map(|security| permissions_value(Permissions::from_bits(security.permissions())));
    out.push(field("permissions", permissions.unwrap_or(Value::Null)));
    Ok(out)
}

fn info_value(info: &Info) -> Value {
    let mut fields = Vec::new();
    for (key, value) in [
        ("title", &info.title),
        ("author", &info.author),
        ("subject", &info.subject),
        ("keywords", &info.keywords),
        ("creator", &info.creator),
        ("producer", &info.producer),
        ("creationDate", &info.creation_date),
        ("modDate", &info.mod_date),
    ] {
        if let Some(value) = value {
            fields.push(field(key, Value::String(value.clone())));
        }
    }
    Value::Object(fields)
}

const PERMISSION_NAMES: [&str; 8] = [
    "print",
    "modify",
    "copy",
    "annotate",
    "fillForms",
    "accessibility",
    "assemble",
    "printHighQuality",
];

fn permissions_value(p: Permissions) -> Value {
    let flags = [
        p.print,
        p.modify,
        p.copy,
        p.annotate,
        p.fill_forms,
        p.accessibility,
        p.assemble,
        p.print_high_quality,
    ];
    Value::Object(
        PERMISSION_NAMES
            .iter()
            .zip(flags)
            .map(|(name, flag)| field(name, Value::Bool(flag)))
            .collect(),
    )
}

fn rect_value(r: &Rect) -> Value {
    Value::Array(vec![num(r.llx), num(r.lly), num(r.urx), num(r.ury)])
}

fn page_value(page: &Page) -> Value {
    // The displayed size: the crop box, turned by /Rotate.
    let (w, h) = (
        (page.crop_box.urx - page.crop_box.llx).abs(),
        (page.crop_box.ury - page.crop_box.lly).abs(),
    );
    let (width, height) = if page.rotate % 180 == 90 {
        (h, w)
    } else {
        (w, h)
    };
    Value::Object(vec![
        field("width", num(width)),
        field("height", num(height)),
        field("rotate", num(f64::from(page.rotate))),
        field("mediaBox", rect_value(&page.media_box)),
        field("cropBox", rect_value(&page.crop_box)),
    ])
}

fn pages(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    Ok(Value::Array(doc.pages()?.iter().map(page_value).collect()))
}

/// A bookmark as otf-pdf writes it: an explicit `[page /Fit]` destination.
fn outline_item(
    bookmark: &Bookmark,
    pages: &[Option<otf_pdf::ObjId>],
) -> std::result::Result<OutlineItem, PdfError> {
    let dest = match bookmark.page {
        None => None,
        Some(index) => {
            let id = pages.get(index).copied().flatten().ok_or(PdfError::syntax(
                0,
                "a bookmark's page index is out of range",
            ))?;
            Some(Object::Array(vec![
                Object::Ref(id),
                Object::Name(otf_pdf::Name::new(b"Fit".to_vec())),
            ]))
        }
    };
    Ok(OutlineItem {
        id: None,
        title: bookmark.title.clone(),
        dest,
        action: None,
        open: bookmark.open,
        children: bookmark
            .children
            .iter()
            .map(|child| outline_item(child, pages))
            .collect::<std::result::Result<_, _>>()?,
        extra: otf_pdf::Dict::new(),
    })
}

fn xmp(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    Ok(match doc.xmp_metadata()? {
        Some(bytes) => Value::String(String::from_utf8_lossy(&bytes).into_owned()),
        None => Value::Undefined,
    })
}

fn destinations(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    let pages: Vec<_> = doc.pages()?.iter().map(|p| p.id).collect();
    let named = doc.named_destinations()?;
    Ok(Value::Array(
        named
            .iter()
            .map(|(name, dest)| {
                let mut fields = vec![field(
                    "name",
                    Value::String(String::from_utf8_lossy(name).into_owned()),
                )];
                if let Some(page) = dest_page(dest, &pages, &named, 0) {
                    fields.push(field("page", num(page as f64)));
                }
                Value::Object(fields)
            })
            .collect(),
    ))
}

fn text(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    Ok(Value::String(TextExtractor::new(doc).text()?))
}

fn page_text(doc: &Document, args: &[Value]) -> std::result::Result<Value, PdfError> {
    let index = args.first().and_then(Value::as_number).unwrap_or(0.0) as usize;
    let with_chars = matches!(args.get(1), Some(Value::Bool(true)));
    let page = TextExtractor::new(doc).page(index)?;
    let lines = page
        .lines
        .iter()
        .map(|line| {
            let words = line
                .words()
                .into_iter()
                .map(|w| {
                    Value::Object(vec![
                        field("text", Value::String(w.text)),
                        field("bbox", rect_value(&w.bbox)),
                    ])
                })
                .collect();
            let mut fields = vec![
                field("text", Value::String(line.text.clone())),
                field("bbox", rect_value(&line.bbox)),
                field("words", Value::Array(words)),
            ];
            if with_chars {
                let chars = line
                    .chars
                    .iter()
                    .map(|c| {
                        Value::Object(vec![
                            field("text", Value::String(c.text.clone())),
                            field(
                                "origin",
                                Value::Array(vec![num(c.origin.0), num(c.origin.1)]),
                            ),
                            field("end", Value::Array(vec![num(c.end.0), num(c.end.1)])),
                            field("size", num(c.size)),
                            field("font", Value::String(c.font.clone())),
                            field("invisible", Value::Bool(c.invisible)),
                        ])
                    })
                    .collect();
                fields.push(field("chars", Value::Array(chars)));
            }
            Value::Object(fields)
        })
        .collect();
    Ok(Value::Object(vec![
        field("text", Value::String(page.text())),
        field("lines", Value::Array(lines)),
    ]))
}

fn outlines(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    let pages: Vec<_> = doc.pages()?.iter().map(|p| p.id).collect();
    let named = doc.named_destinations()?;
    let items = doc.outlines()?;
    Ok(Value::Array(
        items
            .iter()
            .map(|item| outline_value(item, &pages, &named))
            .collect(),
    ))
}

fn outline_value(
    item: &OutlineItem,
    pages: &[Option<otf_pdf::ObjId>],
    named: &[(Vec<u8>, Object)],
) -> Value {
    let mut fields = vec![field("title", Value::String(item.title.clone()))];
    let dest = item
        .dest
        .as_ref()
        .or_else(|| item.action.as_ref().and_then(|a| a.get(b"D")));
    if let Some(page) = dest.and_then(|d| dest_page(d, pages, named, 0)) {
        fields.push(field("page", num(page as f64)));
    }
    fields.push(field("open", Value::Bool(item.open)));
    fields.push(field(
        "children",
        Value::Array(
            item.children
                .iter()
                .map(|child| outline_value(child, pages, named))
                .collect(),
        ),
    ));
    Value::Object(fields)
}

/// The page index a destination names: an explicit `[page /Fit …]`, or a
/// name looked up in the document's named destinations (which may hold the
/// array itself or `<< /D [...] >>`).
fn dest_page(
    dest: &Object,
    pages: &[Option<otf_pdf::ObjId>],
    named: &[(Vec<u8>, Object)],
    depth: u8,
) -> Option<usize> {
    if depth > 4 {
        return None;
    }
    match dest {
        Object::Array(items) => match items.first()? {
            Object::Ref(id) => pages.iter().position(|p| *p == Some(*id)),
            _ => None,
        },
        Object::Dict(dict) => dest_page(dict.get(b"D")?, pages, named, depth + 1),
        Object::String(s) => lookup_named(&s.bytes, pages, named, depth),
        Object::Name(n) => lookup_named(n.as_bytes(), pages, named, depth),
        _ => None,
    }
}

fn lookup_named(
    name: &[u8],
    pages: &[Option<otf_pdf::ObjId>],
    named: &[(Vec<u8>, Object)],
    depth: u8,
) -> Option<usize> {
    let (_, dest) = named.iter().find(|(n, _)| n == name)?;
    dest_page(dest, pages, named, depth + 1)
}

fn attachments(doc: &Document, _args: &[Value]) -> std::result::Result<Value, PdfError> {
    Ok(Value::Array(
        doc.attachments()?
            .iter()
            .map(|a| {
                let mut fields = vec![field("name", Value::String(a.name.clone()))];
                for (key, value) in [
                    ("filename", &a.filename),
                    ("description", &a.description),
                    ("type", &a.mime_type),
                ] {
                    if let Some(value) = value {
                        fields.push(field(key, Value::String(value.clone())));
                    }
                }
                if let Some(size) = a.size {
                    fields.push(field("size", num(size as f64)));
                }
                Value::Object(fields)
            })
            .collect(),
    ))
}

fn attachment(doc: &Document, args: &[Value]) -> std::result::Result<Value, PdfError> {
    let name = args.first().and_then(Value::as_str).unwrap_or_default();
    let Some(found) = doc.attachments()?.into_iter().find(|a| a.name == name) else {
        return Ok(Value::Undefined);
    };
    Ok(Value::Bytes(doc.attachment_data(&found)?))
}

// ---- results ----------------------------------------------------------------

fn ok(value: Value) -> Value {
    Value::Object(vec![field("ok", value)])
}

fn num(n: f64) -> Value {
    Value::Number(n)
}

fn field(key: &str, value: Value) -> (String, Value) {
    (key.to_string(), value)
}

/// `{ error, message }` with a stable name `pdf.js` maps to an `ERR_PDF_*`
/// code. A budget refusal while the cancel flag is set is an abort.
fn failure(error: &PdfError, cancel: Option<&AtomicBool>) -> Value {
    let message = error.to_string();
    let (code, message) = match error {
        PdfError::Limit(LimitKind::Budget)
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) =>
        {
            ("aborted", message)
        }
        PdfError::Limit(kind) => {
            return Value::Object(vec![
                field("error", Value::String("limit".into())),
                field("limit", Value::String(limit_name(*kind).into())),
                field("message", Value::String(message)),
            ]);
        }
        PdfError::InvalidPassword => ("invalid_password", message),
        PdfError::UnsupportedEncryption(m) if m.starts_with(REFUSED) => {
            ("incremental_refused", m[REFUSED.len()..].to_string())
        }
        PdfError::UnsupportedEncryption(_) => ("unsupported_encryption", message),
        PdfError::Source(_) => ("source", message),
        _ => ("malformed", message),
    };
    Value::Object(vec![
        field("error", Value::String(code.into())),
        field("message", Value::String(message)),
    ])
}

fn limit_name(kind: LimitKind) -> &'static str {
    match kind {
        LimitKind::Depth => "depth",
        LimitKind::Objects => "objects",
        LimitKind::DecodedSize => "decodedSize",
        LimitKind::CompressionRatio => "compressionRatio",
        LimitKind::XrefSections => "xrefSections",
        LimitKind::ObjectSize => "objectSize",
        LimitKind::Pages => "pages",
        LimitKind::Budget => "budget",
    }
}

// ---- arguments --------------------------------------------------------------

fn mint(ids: &Rc<RefCell<u64>>) -> u64 {
    let mut id = ids.borrow_mut();
    *id += 1;
    *id
}

fn closed() -> OpError {
    OpError::new(ExceptionClass::TypeError, "this PDF has been closed")
}

fn lookup(reg: &Handles, args: &[Value]) -> std::result::Result<Rc<PdfHandle>, OpError> {
    let id = number_arg(args, 0, "handle")?;
    reg.borrow().get(&id).cloned().ok_or_else(closed)
}

fn token_arg(
    tokens: &Tokens,
    value: Option<&Value>,
) -> std::result::Result<Option<Arc<AtomicBool>>, OpError> {
    match value {
        None | Some(Value::Undefined | Value::Null) => Ok(None),
        Some(Value::Number(n)) => Ok(tokens.borrow().get(&(*n as u64)).cloned()),
        Some(_) => Err(OpError::type_error("a PDF cancel token must be a number")),
    }
}

fn take_bytes(
    args: &mut [Value],
    index: usize,
    what: &str,
) -> std::result::Result<Vec<u8>, OpError> {
    match args
        .get_mut(index)
        .map(|v| std::mem::replace(v, Value::Undefined))
    {
        Some(Value::Bytes(bytes)) => Ok(bytes),
        _ => Err(OpError::type_error(format!("a PDF {what} must be bytes"))),
    }
}

fn string_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<String, OpError> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| OpError::type_error(format!("a PDF {what} must be a string")))
}

fn number_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<u64, OpError> {
    match args.get(index) {
        Some(Value::Number(n)) if n.is_finite() && *n >= 0.0 => Ok(*n as u64),
        _ => Err(OpError::type_error(format!(
            "a PDF {what} must be a number"
        ))),
    }
}

fn get<'a>(fields: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
        .filter(|v| !matches!(v, Value::Undefined))
}

fn parse_open(value: Option<&Value>) -> std::result::Result<Open, OpError> {
    let mut open = Open {
        password: Vec::new(),
        strictness: Strictness::Lenient,
        max_decoded_size: MAX_DECODED_SIZE,
    };
    let Some(Value::Object(fields)) = value else {
        return Ok(open);
    };
    if let Some(Value::String(password)) = get(fields, "password") {
        open.password = password.as_bytes().to_vec();
    }
    if let Some(Value::Bool(true)) = get(fields, "strict") {
        open.strictness = Strictness::Strict;
    }
    if let Some(Value::Number(n)) = get(fields, "maxDecodedSize") {
        open.max_decoded_size = *n as u64;
    }
    Ok(open)
}

fn indices(value: Option<&Value>) -> std::result::Result<Vec<usize>, OpError> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Number(n) if n.fract() == 0.0 && *n >= 0.0 => Ok(*n as usize),
                _ => Err(OpError::type_error(
                    "a page index must be a non-negative integer",
                )),
            })
            .collect(),
        _ => Err(OpError::type_error("pages must be an array of indices")),
    }
}

fn parse_steps(value: Option<&Value>) -> std::result::Result<Vec<Step>, OpError> {
    match value {
        None | Some(Value::Undefined | Value::Null) => Ok(Vec::new()),
        Some(Value::Array(steps)) => steps.iter().map(parse_step).collect(),
        Some(_) => Err(OpError::type_error("PDF steps must be an array")),
    }
}

fn parse_step(value: &Value) -> std::result::Result<Step, OpError> {
    let Value::Array(parts) = value else {
        return Err(OpError::type_error("a PDF step must be an array"));
    };
    let name = parts.first().and_then(Value::as_str).unwrap_or_default();
    let arg = |i: usize| parts.get(i);
    Ok(match name {
        "select" => Step::Select(indices(arg(1))?),
        "delete" => Step::Delete(indices(arg(1))?),
        "rotate" => {
            let degrees = arg(2).and_then(Value::as_number).unwrap_or(f64::NAN);
            if degrees.fract() != 0.0 || degrees % 90.0 != 0.0 {
                return Err(OpError::range_error("rotation must be a multiple of 90"));
            }
            Step::Rotate(indices(arg(1))?, degrees as i32)
        }
        "crop" | "mediaBox" => {
            let rect = match arg(2) {
                Some(Value::Array(items)) if items.len() == 4 => {
                    let n: Vec<f64> = items.iter().filter_map(Value::as_number).collect();
                    (n.len() == 4 && n.iter().all(|v| v.is_finite())).then(|| Rect {
                        llx: n[0],
                        lly: n[1],
                        urx: n[2],
                        ury: n[3],
                    })
                }
                _ => None,
            }
            .ok_or_else(|| OpError::type_error("a crop box must be [left, bottom, right, top]"))?;
            if name == "crop" {
                Step::Crop(indices(arg(1))?, rect)
            } else {
                Step::MediaBox(indices(arg(1))?, rect)
            }
        }
        "setOutlines" => match arg(1) {
            Some(Value::Array(items)) => Step::SetOutlines(
                items
                    .iter()
                    .map(|item| parse_bookmark(item, 0))
                    .collect::<std::result::Result<_, _>>()?,
            ),
            _ => return Err(OpError::type_error("setOutlines takes an array")),
        },
        "append" => {
            let Some(Value::Object(other)) = arg(1) else {
                return Err(OpError::type_error("append takes a PDF"));
            };
            let handle = number_arg(
                &[get(other, "handle").cloned().unwrap_or(Value::Undefined)],
                0,
                "handle",
            )?;
            let steps = parse_steps(get(other, "steps"))?;
            let pages = match arg(2) {
                None | Some(Value::Undefined | Value::Null) => None,
                value => Some(indices(value)?),
            };
            Step::Append(Box::new(Nested { handle, steps }), pages)
        }
        "setInfo" => {
            let Some(Value::Object(fields)) = arg(1) else {
                return Err(OpError::type_error("setInfo takes an object"));
            };
            Step::SetInfo(
                fields
                    .iter()
                    .filter_map(|(k, v)| match v {
                        Value::String(s) => Some((k.clone(), Some(s.clone()))),
                        Value::Null => Some((k.clone(), None)),
                        _ => None,
                    })
                    .collect(),
            )
        }
        "addAttachment" => {
            let name = arg(1)
                .and_then(Value::as_str)
                .ok_or_else(|| OpError::type_error("an attachment name must be a string"))?;
            let data = arg(2)
                .and_then(Value::as_bytes)
                .ok_or_else(|| OpError::type_error("attachment data must be bytes"))?;
            let mut options = AttachmentOptions::default();
            if let Some(Value::Object(fields)) = arg(3) {
                options.mime_type = get(fields, "type").and_then(Value::as_str).map(Into::into);
                options.description = get(fields, "description")
                    .and_then(Value::as_str)
                    .map(Into::into);
            }
            Step::AddAttachment {
                name: name.to_string(),
                data: data.to_vec(),
                options,
            }
        }
        "removeAttachment" => Step::RemoveAttachment(
            arg(1)
                .and_then(Value::as_str)
                .ok_or_else(|| OpError::type_error("an attachment name must be a string"))?
                .to_string(),
        ),
        "sanitize" => Step::Sanitize(sanitize_options(arg(1))),
        other => return Err(OpError::type_error(format!("unknown PDF step {other:?}"))),
    })
}

fn sanitize_options(value: Option<&Value>) -> SanitizeOptions {
    let mut options = SanitizeOptions::default();
    if let Some(Value::Object(fields)) = value {
        for (key, slot) in [
            ("javascript", &mut options.javascript),
            ("externalActions", &mut options.external_actions),
            ("links", &mut options.links),
            ("attachments", &mut options.attachments),
            ("xfa", &mut options.xfa),
            ("multimedia", &mut options.multimedia),
            ("metadata", &mut options.metadata),
        ] {
            if let Some(Value::Bool(b)) = get(fields, key) {
                *slot = *b;
            }
        }
    }
    options
}

/// Bookmarks nest at most as deep as otf-pdf reads them back.
const MAX_BOOKMARK_DEPTH: usize = 256;

fn parse_bookmark(value: &Value, depth: usize) -> std::result::Result<Bookmark, OpError> {
    if depth >= MAX_BOOKMARK_DEPTH {
        return Err(OpError::range_error("bookmarks nest too deeply"));
    }
    let Value::Object(fields) = value else {
        return Err(OpError::type_error("a bookmark must be an object"));
    };
    let title = get(fields, "title")
        .and_then(Value::as_str)
        .ok_or_else(|| OpError::type_error("a bookmark needs a title"))?
        .to_string();
    let page = match get(fields, "page") {
        None | Some(Value::Null) => None,
        Some(Value::Number(n)) if n.fract() == 0.0 && *n >= 0.0 => Some(*n as usize),
        Some(_) => return Err(OpError::type_error("a bookmark's page must be an index")),
    };
    let children = match get(fields, "children") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| parse_bookmark(item, depth + 1))
            .collect::<std::result::Result<_, _>>()?,
        Some(_) => return Err(OpError::type_error("bookmark children must be an array")),
    };
    Ok(Bookmark {
        title,
        page,
        open: matches!(get(fields, "open"), Some(Value::Bool(true))),
        children,
    })
}

fn parse_save(value: Option<&Value>) -> std::result::Result<Save, OpError> {
    let mut save = Save::default();
    let Some(Value::Object(fields)) = value else {
        return Ok(save);
    };
    if let Some(Value::Bool(b)) = get(fields, "incremental") {
        save.incremental = *b;
    }
    if let Some(Value::Bool(b)) = get(fields, "objectStreams") {
        save.object_streams = *b;
    }
    if let Some(Value::Bool(b)) = get(fields, "compress") {
        save.compress = *b;
    }
    match get(fields, "version").and_then(Value::as_str) {
        None => {}
        Some("1.7") => save.version = Version::new(1, 7),
        Some("2.0") => save.version = Version::new(2, 0),
        Some(other) => {
            return Err(OpError::range_error(format!(
                "a PDF is written as version \"1.7\" or \"2.0\", not {other:?}"
            )));
        }
    }
    if let Some(Value::Object(encrypt)) = get(fields, "encrypt") {
        let password = |key: &str| {
            get(encrypt, key)
                .and_then(Value::as_str)
                .map(|s| s.as_bytes().to_vec())
        };
        let mut permissions = Permissions::ALL;
        if let Some(Value::Object(given)) = get(encrypt, "permissions") {
            let slots = [
                &mut permissions.print,
                &mut permissions.modify,
                &mut permissions.copy,
                &mut permissions.annotate,
                &mut permissions.fill_forms,
                &mut permissions.accessibility,
                &mut permissions.assemble,
                &mut permissions.print_high_quality,
            ];
            for (name, slot) in PERMISSION_NAMES.iter().zip(slots) {
                if let Some(Value::Bool(b)) = get(given, name) {
                    *slot = *b;
                }
            }
        }
        let owner_password = password("ownerPassword")
            .filter(|p| !p.is_empty())
            .ok_or_else(|| OpError::type_error("encrypt needs a non-empty ownerPassword"))?;
        save.encryption = Some(Encryption {
            user_password: password("userPassword").unwrap_or_default(),
            owner_password,
            permissions,
            encrypt_metadata: !matches!(get(encrypt, "encryptMetadata"), Some(Value::Bool(false))),
        });
    }
    Ok(save)
}

fn require(fs: &Option<Arc<dyn FileSystem>>) -> std::result::Result<Arc<dyn FileSystem>, OpError> {
    fs.clone().ok_or_else(|| {
        OpError::new(
            ExceptionClass::Error,
            "filesystem is unavailable (no FileSystem provider configured)",
        )
        .with_code(ErrorCode::ProviderUnavailable)
    })
}

fn map_err(e: ProviderError) -> OpError {
    OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
}

/// Fuzz entry: arbitrary bytes through what `PDF.open` and the reads do —
/// the open, the summary, pages, text, bookmarks and named destinations
/// (whose lookups recurse), attachments and XMP — then a save with edits and a
/// sanitize report (see [`crate::fuzz`]). otf-pdf fuzzes its parser
/// upstream; this reaches the runtime's own walk over what it returns.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_open(bytes: &[u8]) {
    // Small enough that a bomb or a huge object stops each iteration early.
    let open = Open {
        password: Vec::new(),
        strictness: Strictness::Lenient,
        max_decoded_size: 1 << 20,
    };
    let source = Shared(Arc::new(bytes.to_vec()));
    let Ok(doc) = Document::open(source.clone(), open.options(None)) else {
        return;
    };
    let _ = summary(&doc);
    for read in [pages, text, outlines, destinations, attachments, xmp] {
        let _ = read(&doc, &[]);
    }
    let _ = page_text(&doc, &[Value::Number(0.0), Value::Bool(true)]);
    let count = doc.page_count().unwrap_or(0);
    let mut steps = vec![ResolvedStep::Plain(Step::Sanitize(SanitizeOptions::ALL))];
    if count > 0 {
        steps.push(ResolvedStep::Plain(Step::Select(vec![count - 1, 0])));
        steps.push(ResolvedStep::Plain(Step::SetOutlines(vec![Bookmark {
            title: "x".into(),
            page: Some(0),
            open: true,
            children: Vec::new(),
        }])));
    }
    let job = Job {
        bytes: source,
        open,
        doc: Arc::new(doc),
        steps,
    };
    let _ = build(job, &None).and_then(|doc| doc.save(&SaveOptions::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> Value {
        Value::String(text.to_string())
    }

    fn n(value: f64) -> Value {
        Value::Number(value)
    }

    fn list(values: &[f64]) -> Value {
        Value::Array(values.iter().copied().map(n).collect())
    }

    /// A minimal PDF with `count` empty letter-size pages and a valid xref.
    fn minimal(count: usize) -> Vec<u8> {
        Document::open(page_source(count), OpenOptions::default())
            .and_then(|doc| doc.save(&SaveOptions::default()))
            .expect("write a minimal PDF")
    }

    fn page_source(count: usize) -> Vec<u8> {
        let kids: Vec<String> = (0..count).map(|i| format!("{} 0 R", i + 3)).collect();
        let mut out = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [{}] /Count {count} /MediaBox [0 0 612 792] >> endobj\n",
            kids.join(" ")
        );
        for i in 0..count {
            out.push_str(&format!(
                "{} 0 obj << /Type /Page /Parent 2 0 R >> endobj\n",
                i + 3
            ));
        }
        out.into_bytes()
    }

    #[test]
    fn parses_every_step() {
        let steps = parse_steps(Some(&Value::Array(vec![
            Value::Array(vec![s("select"), list(&[2.0, 0.0])]),
            Value::Array(vec![s("delete"), list(&[1.0])]),
            Value::Array(vec![s("rotate"), list(&[0.0]), n(-90.0)]),
            Value::Array(vec![
                s("crop"),
                list(&[0.0]),
                list(&[0.0, 0.0, 100.0, 200.0]),
            ]),
            Value::Array(vec![
                s("append"),
                Value::Object(vec![
                    field("handle", n(7.0)),
                    field("steps", Value::Array(vec![])),
                ]),
                Value::Null,
            ]),
            Value::Array(vec![
                s("setInfo"),
                Value::Object(vec![field("title", s("T")), field("author", Value::Null)]),
            ]),
            Value::Array(vec![s("removeAttachment"), s("a.txt")]),
            Value::Array(vec![
                s("sanitize"),
                Value::Object(vec![field("links", Value::Bool(true))]),
            ]),
        ])))
        .expect("parse");
        assert_eq!(steps[0], Step::Select(vec![2, 0]));
        assert_eq!(steps[1], Step::Delete(vec![1]));
        assert_eq!(steps[2], Step::Rotate(vec![0], -90));
        assert!(matches!(&steps[3], Step::Crop(p, r) if p == &[0] && r.ury == 200.0));
        assert!(matches!(&steps[4], Step::Append(nested, None) if nested.handle == 7));
        assert_eq!(
            steps[5],
            Step::SetInfo(vec![
                ("title".into(), Some("T".into())),
                ("author".into(), None)
            ])
        );
        assert_eq!(steps[6], Step::RemoveAttachment("a.txt".into()));
        assert!(matches!(&steps[7], Step::Sanitize(o) if o.links && o.javascript));
    }

    #[test]
    fn parses_bookmarks_and_media_boxes() {
        let step = parse_step(&Value::Array(vec![
            s("setOutlines"),
            Value::Array(vec![Value::Object(vec![
                field("title", s("A")),
                field("page", n(1.0)),
                field("open", Value::Bool(true)),
                field(
                    "children",
                    Value::Array(vec![Value::Object(vec![field("title", s("B"))])]),
                ),
            ])]),
        ]))
        .expect("parse");
        let Step::SetOutlines(items) = step else {
            panic!("not outlines: {step:?}");
        };
        assert_eq!((items[0].page, items[0].open), (Some(1), true));
        assert_eq!(
            (items[0].children[0].page, items[0].children[0].open),
            (None, false)
        );

        let boxed = parse_step(&Value::Array(vec![
            s("mediaBox"),
            list(&[0.0]),
            list(&[0.0, 0.0, 595.0, 842.0]),
        ]))
        .expect("parse");
        assert!(matches!(boxed, Step::MediaBox(p, r) if p == [0] && r.urx == 595.0));

        // Nesting past the bound is refused rather than recursed into.
        let mut deep = Value::Object(vec![field("title", s("x"))]);
        for _ in 0..MAX_BOOKMARK_DEPTH {
            deep = Value::Object(vec![
                field("title", s("x")),
                field("children", Value::Array(vec![deep])),
            ]);
        }
        assert!(parse_bookmark(&deep, 0).is_err());
    }

    #[test]
    fn refuses_bad_steps() {
        for bad in [
            Value::Array(vec![s("rotate"), list(&[0.0]), n(45.0)]),
            Value::Array(vec![s("select"), list(&[-1.0])]),
            Value::Array(vec![s("crop"), list(&[0.0]), list(&[0.0, 0.0])]),
            Value::Array(vec![s("explode")]),
        ] {
            assert!(parse_step(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn parses_save_options() {
        let save = parse_save(Some(&Value::Object(vec![
            field("incremental", Value::Bool(true)),
            field("version", s("2.0")),
            field(
                "encrypt",
                Value::Object(vec![
                    field("userPassword", s("u")),
                    field("ownerPassword", s("o")),
                    field(
                        "permissions",
                        Value::Object(vec![field("copy", Value::Bool(false))]),
                    ),
                ]),
            ),
        ])))
        .expect("parse");
        assert!(save.incremental);
        assert_eq!(save.version, Version::new(2, 0));
        let encryption = save.encryption.expect("encryption");
        assert_eq!(encryption.owner_password, b"o");
        assert!(!encryption.permissions.copy && encryption.permissions.print);
        assert!(encryption.encrypt_metadata);

        let no_owner = Value::Object(vec![field(
            "encrypt",
            Value::Object(vec![field("userPassword", s("u"))]),
        )]);
        assert!(parse_save(Some(&no_owner)).is_err());
        let bad_version = Value::Object(vec![field("version", s("1.4"))]);
        assert!(parse_save(Some(&bad_version)).is_err());
    }

    #[test]
    fn open_defaults_lower_the_decoded_size() {
        let open = parse_open(None).expect("parse");
        assert_eq!(open.max_decoded_size, MAX_DECODED_SIZE);
        assert!(MAX_DECODED_SIZE < Limits::default().max_decoded_size);
    }

    #[test]
    fn failures_carry_stable_names() {
        let name = |v: Value| match v {
            Value::Object(fields) => get(&fields, "error")
                .and_then(Value::as_str)
                .map(String::from),
            _ => None,
        };
        assert_eq!(
            name(failure(&PdfError::InvalidPassword, None)).as_deref(),
            Some("invalid_password")
        );
        assert_eq!(
            name(failure(&PdfError::NoXref, None)).as_deref(),
            Some("malformed")
        );
        assert_eq!(
            name(failure(&PdfError::Limit(LimitKind::DecodedSize), None)).as_deref(),
            Some("limit")
        );
        let flag = AtomicBool::new(true);
        assert_eq!(
            name(failure(&PdfError::Limit(LimitKind::Budget), Some(&flag))).as_deref(),
            Some("aborted")
        );
        let refusal = refused(
            PdfError::syntax(0, "no startxref to append to; save in full"),
            true,
        );
        assert_eq!(
            name(failure(&refusal, None)).as_deref(),
            Some("incremental_refused")
        );
        // The same message on a full save is not a refusal.
        let full = refused(PdfError::syntax(0, "x; save in full"), false);
        assert_eq!(name(failure(&full, None)).as_deref(), Some("malformed"));
    }

    #[test]
    fn a_set_flag_stops_the_work() {
        let flag = Arc::new(AtomicBool::new(true));
        let open = parse_open(None).expect("parse");
        let result = Document::open(minimal(3), open.options(Some(flag)))
            .and_then(|doc| TextExtractor::new(&doc).text());
        assert!(
            matches!(result, Err(PdfError::Limit(LimitKind::Budget))),
            "{result:?}"
        );
    }

    /// Deterministic bytes; nothing here encrypts, but a save takes one.
    struct Counting;

    impl Entropy for Counting {
        fn fill(&self, dest: &mut [u8]) -> std::result::Result<(), ProviderError> {
            for (i, b) in dest.iter_mut().enumerate() {
                *b = i as u8;
            }
            Ok(())
        }
    }

    #[test]
    fn steps_apply_to_a_fresh_open() {
        let bytes = Shared(Arc::new(minimal(3)));
        let open = parse_open(None).expect("parse");
        let doc = Arc::new(Document::open(bytes.clone(), open.options(None)).expect("open"));
        let job = Job {
            bytes,
            open,
            doc: doc.clone(),
            steps: vec![
                ResolvedStep::Plain(Step::Select(vec![2, 0])),
                ResolvedStep::Plain(Step::Rotate(vec![0], 90)),
            ],
        };
        let entropy = Arc::new(Drawn {
            entropy: Arc::new(Counting),
            failed: std::sync::Mutex::new(None),
        });
        let out = run_save(job, Save::default(), None, entropy).expect("save");
        let saved = Document::open(out, OpenOptions::default()).expect("reopen");
        assert_eq!(saved.page_count().expect("count"), 2);
        assert_eq!(saved.pages().expect("pages")[0].rotate, 90);
        // The open document is untouched.
        assert_eq!(doc.page_count().expect("count"), 3);
    }

    #[test]
    fn outline_destinations_resolve_to_pages() {
        let id = otf_pdf::ObjId::new(5, 0);
        let pages = [None, Some(id)];
        let explicit = Object::Array(vec![Object::Ref(id)]);
        assert_eq!(dest_page(&explicit, &pages, &[], 0), Some(1));
        let named = [(b"intro".to_vec(), explicit.clone())];
        let by_name = Object::String(otf_pdf::PdfString::literal(b"intro".to_vec()));
        assert_eq!(dest_page(&by_name, &pages, &named, 0), Some(1));
        // A name that refers to itself ends rather than recursing.
        let looped = [(
            b"x".to_vec(),
            Object::Name(otf_pdf::Name::new(b"x".to_vec())),
        )];
        assert_eq!(
            dest_page(
                &Object::Name(otf_pdf::Name::new(b"x".to_vec())),
                &pages,
                &looped,
                0
            ),
            None
        );
    }
}
