//! Host ops backing `runtime:images` (DECISIONS.md D159), on otf-pixels.
//!
//! A pipeline arrives whole: the source (bytes, or a path the `FileSystem`
//! provider reads), the open options, the list of steps the JS chain recorded,
//! and the output. Nothing is held between calls, so there is no handle to
//! free and nothing an agent could name that another agent made.
//!
//! The work runs on the embedder's [`TaskSpawner`] — Pixels is synchronous by
//! design — and its tiles on Pixels' process-wide scheduler, which every agent
//! and worker shares because no output here asks for a pool of its own. Without
//! a spawner the work runs inline.
//!
//! The capability split is by op name rather than by argument, because an op
//! declares what it requires before it runs: bytes in and out need nothing, a
//! `file()` source needs `FileRead`, a `file()` destination `FileWrite`.
//!
//! An image failure resolves as `{ error, message }` with Pixels' stable code
//! (`malformed`, `unsupported`, …) rather than rejecting, so `images.js` maps
//! it to the module's `ERR_IMAGE_*` codes and its error classes in one place.
//! Filesystem failures reject as `runtime:fs` ones do.
//!
//! A call `images.js` gave a signal or a timeout carries a Pixels [`Cancel`]
//! (D255): the op files it under the call's id before it awaits anything,
//! `image_cancel` cancels it, and a guard removes it when the call settles.
//! The run stops itself at its next check and settles as `cancelled`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{FileSystem, ProviderError, TaskSpawner};
use otf_pixels::{
    AffineOptions, Background, Blend, Cancel, Channel, ChannelLayout, ChromaSubsampling, Codec,
    Density, EncodeOptions, Extend, ExtendWith, Filter, Fit, Format, Image, ImageDescriptor,
    Interpolator, Kernel, Limits, Modulate, OpenOptions, Overlay, PixelFormat, PixelsError,
    Position, ResizeOptions, ResolutionUnit, RotateOptions, SampleKind, TiffCodec, Trim,
};

use crate::Result;
use crate::blocking::offload;

/// Where the encoded bytes come from.
enum Source {
    Bytes(Vec<u8>),
    File(String),
}

/// How the source's bytes become an image, and the pixel limit that applies
/// to it however it is made.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    options: OpenOptions,
    limits: Limits,
    decode: Decode,
    /// sharp's `page`: a TIFF's page, or an animation's first frame. Which
    /// one is known only from the bytes, so it is applied at open.
    page: u32,
    /// sharp's `pages`: `None` unset, `Some(None)` all (`-1`), else a count.
    pages: Option<Option<u32>>,
    /// sharp's `timeout`: a deadline on each run, from the terminal call.
    timeout: Option<Duration>,
}

impl Default for Open {
    fn default() -> Self {
        Self {
            options: OpenOptions::default(),
            limits: Limits::default(),
            decode: Decode::Encoded,
            page: 0,
            pages: None,
            timeout: None,
        }
    }
}

impl Open {
    /// The open options for `bytes`: `page` and `pages` go to a TIFF's page
    /// or to an animation's frames, by what the leading bytes are.
    fn options_for(&self, bytes: &[u8]) -> std::result::Result<OpenOptions, PixelsError> {
        let mut options = self.options;
        if TiffCodec.probe(bytes) {
            if matches!(self.pages, Some(count) if count != Some(1)) {
                return Err(PixelsError::unsupported(
                    "a multi-page TIFF is read one page at a time; pass `page` without `pages`",
                ));
            }
            return Ok(options.with_page(self.page));
        }
        if let Some(count) = self.pages {
            // sharp's `animated` is `pages: -1`: more than one frame is animated.
            if count != Some(1) {
                options = options.with_animated(true);
            }
            return Ok(options.with_frames(self.page, count));
        }
        Ok(if self.page > 0 {
            options.with_frames(self.page, None)
        } else {
            options
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Decode {
    /// A file in a format its leading bytes name.
    Encoded,
    /// Packed pixels laid out as given (sharp's `raw`).
    Raw(Layout),
    /// A canvas of one colour; the source bytes are empty (sharp's `create`).
    Create { layout: Layout, fill: [u8; 4] },
}

/// A raw or created image's size and pixel format. It becomes a descriptor
/// only when the pipeline runs, against the image's own `maxPixels`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    width: u32,
    height: u32,
    pixel: PixelFormat,
}

impl Layout {
    fn descriptor(self, limits: &Limits) -> std::result::Result<ImageDescriptor, PixelsError> {
        ImageDescriptor::with_limits(self.width, self.height, self.pixel, limits)
    }
}

/// One transform, as `images.js` records it.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// A zero width or height is "keep the aspect ratio".
    Resize {
        width: u32,
        height: u32,
        options: ResizeOptions,
    },
    Crop {
        left: u32,
        top: u32,
        width: u32,
        height: u32,
    },
    Rotate {
        degrees: f64,
        background: [u8; 4],
    },
    Affine {
        matrix: [f64; 4],
        options: AffineOptions,
    },
    /// One frame of an animation, as a still.
    Frame(u32),
    /// Every frame stacked top to bottom into one still.
    StackFrames,
    Flip,
    Flop,
    Modulate {
        brightness: f32,
        saturation: f32,
        hue: f32,
        lightness: f32,
    },
    Blur(f32),
    Sharpen(f32),
    Flatten([u8; 3]),
    Grayscale,
    ExtractChannel(usize),
    ExtractChannelNamed(Channel),
    Negate {
        alpha: bool,
    },
    Gamma(f32),
    Linear {
        a: [f32; 3],
        b: [f32; 3],
    },
    Threshold {
        level: u8,
        greyscale: bool,
    },
    Tint([u8; 3]),
    Greyscale,
    Recomb3([[f32; 3]; 3]),
    Recomb4([[f32; 4]; 4]),
    EnsureAlpha(f32),
    RemoveAlpha,
    Convolve(Kernel),
    Extend {
        /// Top, right, bottom, left.
        margins: [u32; 4],
        with: ExtendWith,
        background: [u8; 4],
    },
    Trim {
        background: Option<[u8; 4]>,
        threshold: f32,
        line_art: bool,
    },
    /// Overlays in drawing order, the first lowest.
    Composite(Vec<OverlayStep>),
}

/// One `composite` overlay: a whole pipeline of its own, already in bytes —
/// `images.js` reads a `file()` overlay through `runtime:fs` before the call,
/// so the op's capability stays the one its base source declares.
#[derive(Debug, Clone, PartialEq)]
struct OverlayStep {
    bytes: Vec<u8>,
    open: Open,
    steps: Vec<Step>,
    blend: Blend,
    gravity: Position,
    offset: Option<(i64, i64)>,
    tile: bool,
}

/// The encode target; `None` keeps the source's format. `xmp` and `density`
/// replace what the source carries.
#[derive(Debug, Clone, Default, PartialEq)]
struct Output {
    format: Option<Format>,
    /// For raw output: the sample type written. `None` is 8-bit, as sharp's
    /// `raw()` defaults to `uchar`.
    depth: Option<SampleKind>,
    options: EncodeOptions,
    xmp: Option<Vec<u8>>,
    density: Option<f64>,
    /// sharp's `loop` and `delay`, for an animated output.
    loop_count: Option<u32>,
    delay: Option<Delay>,
}

#[derive(Debug, Clone, PartialEq)]
enum Delay {
    Every(u32),
    Each(Vec<u32>),
}

/// The `Cancel` of each running call that has a signal, by the id
/// `images.js` gave it. One table per agent.
type Cancels = Rc<RefCell<HashMap<u64, Cancel>>>;

/// Removes a call's entry when the call settles, however it settles.
struct Filed {
    cancels: Cancels,
    id: Option<u64>,
}

impl Drop for Filed {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.cancels.borrow_mut().remove(&id);
        }
    }
}

/// The call's `Cancel`: a deadline if the chain set `timeout`, a flag the
/// signal can raise if `id` is given, both, or neither. Filed before the
/// call awaits anything, so an abort right after the call still lands.
fn file_cancel(
    cancels: &Cancels,
    id: Option<&Value>,
    timeout: Option<Duration>,
) -> std::result::Result<(Cancel, Filed), OpError> {
    let id = match id {
        None | Some(Value::Undefined | Value::Null) => None,
        Some(value) => Some(
            value
                .as_number()
                .filter(|n| n.fract() == 0.0 && *n >= 0.0)
                .ok_or_else(|| OpError::type_error("an image cancel id must be an integer"))?
                as u64,
        ),
    };
    let cancel = match (timeout, id) {
        (Some(timeout), _) => Cancel::after(timeout),
        (None, Some(_)) => Cancel::new(),
        (None, None) => Cancel::never(),
    };
    if let Some(id) = id {
        let mut table = cancels.borrow_mut();
        if table.contains_key(&id) {
            return Err(OpError::type_error("an image cancel id is already in use"));
        }
        table.insert(id, cancel.clone());
    }
    Ok((
        cancel,
        Filed {
            cancels: cancels.clone(),
            id,
        },
    ))
}

/// Registers the `runtime:images` ops.
pub(crate) fn install(
    engine: &mut dyn Engine,
    fs: Option<Arc<dyn FileSystem>>,
    spawner: Option<Arc<dyn TaskSpawner>>,
) -> Result<()> {
    let cancels: Cancels = Rc::default();
    {
        let cancels = cancels.clone();
        engine.register_op(OpDecl::sync("image_cancel", move |args| {
            if let Some(id) = args.first().and_then(Value::as_number)
                && let Some(cancel) = cancels.borrow().get(&(id as u64))
            {
                cancel.cancel();
            }
            Ok(Value::Undefined)
        }))?;
    }

    // (name, source is a file, what it reports)
    type Report = fn(&Image) -> std::result::Result<Value, PixelsError>;
    for (name, from_file, report) in [
        ("image_metadata", false, metadata as Report),
        ("image_metadata_file", true, metadata),
        ("image_stats", false, stats),
        ("image_stats_file", true, stats),
    ] {
        let (fs, spawner, cancels) = (fs.clone(), spawner.clone(), cancels.clone());
        let mut decl = OpDecl::r#async(name, move |mut args| {
            let (fs, spawner) = (fs.clone(), spawner.clone());
            let parsed = parse_common(&mut args, from_file).and_then(|common| {
                let filed = file_cancel(&cancels, args.get(3), common.1.timeout)?;
                Ok((common, filed))
            });
            Box::pin(async move {
                let ((source, open, steps), (cancel, _filed)) = parsed?;
                let bytes = read_source(&fs, source).await?;
                let result = offload(spawner, move || {
                    pipeline(bytes, open, steps, &cancel).and_then(|image| report(&image))
                })
                .await?;
                Ok(result.unwrap_or_else(failure))
            })
        });
        if from_file {
            decl = decl.requires(Capability::FileRead);
        }
        engine.register_op(decl)?;
    }

    for (name, from_file, to_file) in [
        ("image_encode", false, false),
        ("image_encode_file", true, false),
        ("image_write", false, true),
        ("image_write_file", true, true),
    ] {
        let (fs, spawner, cancels) = (fs.clone(), spawner.clone(), cancels.clone());
        let mut decl = OpDecl::r#async(name, move |mut args| {
            let (fs, spawner) = (fs.clone(), spawner.clone());
            // (source, open, steps, output, [destination,] cancel id)
            let cancel_at = if to_file { 5 } else { 4 };
            let parsed = parse_common(&mut args, from_file).and_then(|common| {
                let output = parse_output(args.get(3))?;
                let dest = if to_file {
                    Some(string_arg(&args, 4, "destination")?)
                } else {
                    None
                };
                let filed = file_cancel(&cancels, args.get(cancel_at), common.1.timeout)?;
                Ok((common, output, dest, filed))
            });
            Box::pin(async move {
                let ((source, open, steps), output, dest, (cancel, _filed)) = parsed?;
                let bytes = read_source(&fs, source).await?;
                let encoded =
                    offload(spawner, move || encode(bytes, open, steps, output, &cancel)).await?;
                let encoded = match encoded {
                    Ok(encoded) => encoded,
                    Err(error) => return Ok(failure(error)),
                };
                let (encoded, format) = encoded;
                let value = match dest {
                    Some(path) => {
                        let written = require(&fs)?
                            .write(path, encoded, false)
                            .await
                            .map_err(map_err)?;
                        Value::Number(written as f64)
                    }
                    None => Value::Object(vec![
                        ("bytes".to_string(), Value::Bytes(encoded)),
                        (
                            "format".to_string(),
                            Value::String(format.as_str().to_string()),
                        ),
                    ]),
                };
                Ok(Value::Object(vec![("ok".to_string(), value)]))
            })
        });
        if from_file {
            decl = decl.requires(Capability::FileRead);
        }
        if to_file {
            decl = decl.requires(Capability::FileWrite).target_arg(4);
        }
        engine.register_op(decl)?;
    }
    Ok(())
}

async fn read_source(
    fs: &Option<Arc<dyn FileSystem>>,
    source: Source,
) -> std::result::Result<Vec<u8>, OpError> {
    match source {
        Source::Bytes(bytes) => Ok(bytes),
        Source::File(path) => require(fs)?.read(path).await.map_err(map_err),
    }
}

/// Opens `bytes` and applies `steps`. Errors are carried to the terminal by
/// Pixels itself, so only opening can fail here.
///
/// `cancel` is attached to the image the run starts from, before any step,
/// because `trim`, `stackFrames` and overlays run work while being applied.
fn pipeline(
    bytes: Vec<u8>,
    open: Open,
    steps: Vec<Step>,
    cancel: &Cancel,
) -> std::result::Result<Image, PixelsError> {
    let image = match open.decode {
        Decode::Encoded => {
            let options = open.options_for(&bytes)?;
            Image::from_stream_with(Cursor::new(bytes), options)?
        }
        Decode::Raw(layout) => Image::from_raw(layout.descriptor(&open.limits)?, bytes)?,
        Decode::Create { layout, fill } => {
            let descriptor = layout.descriptor(&open.limits)?;
            let channels = descriptor.pixel.channels();
            let pixels =
                fill[..channels].repeat(descriptor.width as usize * descriptor.height as usize);
            Image::from_raw(descriptor, pixels)?
        }
    };
    let mut image = if cancel.is_never() {
        image
    } else {
        image.with_cancel(cancel)
    };
    for step in steps {
        image = apply(image, step, cancel)?;
    }
    Ok(image)
}

fn apply(image: Image, step: Step, cancel: &Cancel) -> std::result::Result<Image, PixelsError> {
    Ok(match step {
        Step::Resize {
            width,
            height,
            options,
        } => {
            let (width, height) = if width == 0 || height == 0 {
                let current = image.descriptor()?;
                keep_aspect(width, height, current.width, current.height)
            } else {
                (width, height)
            };
            image.resize_with(width, height, options)
        }
        Step::Crop {
            left,
            top,
            width,
            height,
        } => image.crop(left, top, width, height),
        Step::Rotate {
            degrees,
            background,
        } => image.rotate_with(
            degrees,
            RotateOptions::default().with_background(background),
        ),
        Step::Affine { matrix, options } => image.affine(matrix, options),
        Step::Frame(index) => image.frame(index),
        Step::StackFrames => image.into_strip(),
        Step::Flip => image.flip(),
        Step::Flop => image.flop(),
        Step::Modulate {
            brightness,
            saturation,
            hue,
            lightness,
        } => image.modulate(
            Modulate::identity()
                .with_brightness(brightness)?
                .with_saturation(saturation)?
                .with_hue(hue)?
                .with_lightness(lightness)?,
        ),
        Step::Blur(sigma) => image.blur(sigma),
        Step::Sharpen(amount) => image.sharpen(amount),
        Step::Flatten([r, g, b]) => image.flatten(r, g, b),
        Step::Grayscale => {
            let alpha = image.descriptor()?.pixel.has_alpha();
            image.to_pixel_format(if alpha {
                PixelFormat::GrayA8
            } else {
                PixelFormat::Gray8
            })
        }
        Step::ExtractChannel(index) => image.extract_channel(index),
        Step::ExtractChannelNamed(channel) => image.extract_channel_named(channel),
        Step::Negate { alpha } => image.negate(alpha),
        Step::Gamma(exponent) => image.gamma(exponent),
        Step::Linear { a, b } => image.linear_per_channel(a, b),
        Step::Threshold { level, greyscale } => image.threshold(level, greyscale),
        Step::Tint([r, g, b]) => image.tint(r, g, b),
        Step::Greyscale => image.greyscale(),
        Step::Recomb3(matrix) => image.recomb(matrix),
        Step::Recomb4(matrix) => image.recomb_rgba(matrix),
        Step::EnsureAlpha(level) => image.ensure_alpha(level),
        Step::RemoveAlpha => image.remove_alpha(),
        Step::Convolve(kernel) => image.convolve(kernel),
        Step::Extend {
            margins: [top, right, bottom, left],
            with,
            background,
        } => image.extend(
            Extend::new(top, right, bottom, left)
                .extend_with(with)
                .with_background(background),
        ),
        Step::Trim {
            background,
            threshold,
            line_art,
        } => {
            let mut trim = Trim::default()
                .with_threshold(threshold)?
                .line_art(line_art);
            if let Some(rgba) = background {
                trim = trim.with_background(rgba);
            }
            image.trim(trim)
        }
        Step::Composite(overlays) => {
            let overlays = overlays
                .into_iter()
                .map(|overlay| {
                    let mut placed = Overlay::new(pipeline(
                        overlay.bytes,
                        overlay.open,
                        overlay.steps,
                        cancel,
                    )?)
                    .with_blend(overlay.blend)
                    .with_gravity(overlay.gravity)
                    .tiled(overlay.tile);
                    if let Some((x, y)) = overlay.offset {
                        placed = placed.at(x, y);
                    }
                    Ok(placed)
                })
                .collect::<std::result::Result<Vec<_>, PixelsError>>()?;
            image.composite_overlays(overlays)
        }
    })
}

/// The missing side of a resize from the image's own proportions, rounded and
/// never below one pixel.
fn keep_aspect(width: u32, height: u32, src_width: u32, src_height: u32) -> (u32, u32) {
    let scale = |side: u32, num: u32, den: u32| -> u32 {
        let value = (f64::from(side) * f64::from(num) / f64::from(den.max(1))).round();
        (value as u32).max(1)
    };
    if width == 0 {
        (scale(height, src_width, src_height), height)
    } else {
        (width, scale(width, src_height, src_width))
    }
}

fn encode(
    bytes: Vec<u8>,
    open: Open,
    steps: Vec<Step>,
    output: Output,
    cancel: &Cancel,
) -> std::result::Result<(Vec<u8>, Format), PixelsError> {
    let mut image = pipeline(bytes, open, steps, cancel)?;
    let format = match output.format {
        Some(format) => format,
        None => image.metadata()?.format,
    };
    if let Some(xmp) = output.xmp {
        image = image.with_xmp(Some(xmp));
    }
    if let Some(dpi) = output.density {
        image = image.with_density(Some(Density::dpi(dpi)));
    }
    if let Some(count) = output.loop_count {
        image = image.with_loop_count(count);
    }
    match output.delay {
        Some(Delay::Every(ms)) => image = image.with_frame_duration(ms),
        Some(Delay::Each(ms)) => image = image.with_frame_durations(ms),
        None => {}
    }
    if format == Format::Raw {
        let layout = image.descriptor()?.pixel.layout();
        let kind = output.depth.unwrap_or(SampleKind::U8);
        let pixel = PixelFormat::from_parts(layout, kind).ok_or_else(|| {
            PixelsError::invalid_argument(
                "depth",
                format!("no {kind:?} pixel format for a {layout:?} image"),
            )
        })?;
        image = image.to_pixel_format(pixel);
    }
    Ok((image.output(format, output.options).bytes()?, format))
}

fn num(n: impl Into<f64>) -> Value {
    Value::Number(n.into())
}

fn field(key: &str, value: Value) -> (String, Value) {
    (key.to_string(), value)
}

/// The size, pixel format and trim at this point in the chain, then what the
/// source file says about itself, under sharp's names. A field the file does
/// not state is left out, as sharp leaves it out.
fn metadata(image: &Image) -> std::result::Result<Value, PixelsError> {
    let meta = image.metadata()?;
    let animation = match image.animation() {
        Some(animation) => Value::Object(vec![
            (
                "frames".to_string(),
                Value::Number(f64::from(animation.frame_count)),
            ),
            (
                "loop".to_string(),
                Value::Number(f64::from(animation.loop_count)),
            ),
            (
                "durations".to_string(),
                Value::Array(
                    animation
                        .frame_durations_ms
                        .iter()
                        .map(|&ms| Value::Number(f64::from(ms)))
                        .collect(),
                ),
            ),
        ]),
        None => Value::Null,
    };
    let mut fields = vec![
        field("width", num(meta.width)),
        field("height", num(meta.height)),
        field("format", Value::String(meta.format.as_str().to_string())),
        field(
            "pixelFormat",
            Value::String(meta.pixel.as_str().to_string()),
        ),
        field("channels", num(meta.pixel.channels() as u32)),
        field("hasAlpha", Value::Bool(meta.pixel.has_alpha())),
        field("animation", animation),
        field("pages", num(image.pages())),
        field("frames", num(image.frame_count())),
    ];
    // sharp reports where the trimmed image's origin lies in it: negated.
    if let Some((left, top)) = image.trim_offset() {
        fields.push(field("trimOffsetLeft", num(-f64::from(left))));
        fields.push(field("trimOffsetTop", num(-f64::from(top))));
    }
    if let Some(source) = image.source_metadata() {
        fields.push(field("orientation", num(source.orientation.exif())));
        if let Some(bits) = source.bits_per_sample {
            fields.push(field("bitsPerSample", num(bits)));
        }
        if let Some(density) = source.density {
            fields.push(field("density", num(density.x_dpi())));
            let unit = match density.unit {
                ResolutionUnit::Centimetre => "cm",
                _ => "inch",
            };
            fields.push(field("resolutionUnit", Value::String(unit.to_string())));
        }
        fields.push(field("isProgressive", Value::Bool(source.progressive)));
        fields.push(field("isPalette", Value::Bool(source.palette)));
        if let Some(subsampling) = source.chroma_subsampling {
            fields.push(field(
                "chromaSubsampling",
                Value::String(subsampling.to_string()),
            ));
        }
        match source.background {
            Some(Background::Grey(level)) => {
                fields.push(field(
                    "background",
                    Value::Object(vec![field("gray", num(level))]),
                ));
            }
            Some(Background::Rgb(r, g, b)) => {
                fields.push(field(
                    "background",
                    Value::Object(vec![
                        field("r", num(r)),
                        field("g", num(g)),
                        field("b", num(b)),
                    ]),
                ));
            }
            _ => {}
        }
        if let Some(compression) = source.compression {
            fields.push(field("compression", Value::String(compression.to_string())));
        }
        if !source.comments.is_empty() {
            let comments = source
                .comments
                .iter()
                .map(|c| {
                    Value::Object(vec![
                        field("keyword", Value::String(c.keyword.clone())),
                        field("text", Value::String(c.text.clone())),
                    ])
                })
                .collect();
            fields.push(field("comments", Value::Array(comments)));
        }
        fields.push(field("hasProfile", Value::Bool(source.has_profile())));
        for (key, block) in [
            ("icc", &source.icc),
            ("exif", &source.exif),
            ("xmp", &source.xmp),
        ] {
            if let Some(block) = block {
                fields.push(field(key, Value::Bytes(block.to_vec())));
            }
        }
    }
    Ok(Value::Object(vec![field("ok", Value::Object(fields))]))
}

/// sharp's `stats()` of the pipeline's result.
fn stats(image: &Image) -> std::result::Result<Value, PixelsError> {
    let stats = image.stats()?;
    let channels = stats
        .channels
        .iter()
        .map(|c| {
            Value::Object(vec![
                field("min", num(c.min)),
                field("max", num(c.max)),
                field("sum", num(c.sum)),
                field("squaresSum", num(c.squares_sum)),
                field("mean", num(c.mean)),
                field("stdev", num(c.stdev)),
                field("minX", num(c.min_x)),
                field("minY", num(c.min_y)),
                field("maxX", num(c.max_x)),
                field("maxY", num(c.max_y)),
            ])
        })
        .collect();
    let [r, g, b] = stats.dominant;
    Ok(Value::Object(vec![field(
        "ok",
        Value::Object(vec![
            field("channels", Value::Array(channels)),
            field("isOpaque", Value::Bool(stats.is_opaque)),
            field("entropy", num(stats.entropy)),
            field("sharpness", num(stats.sharpness)),
            field(
                "dominant",
                Value::Object(vec![
                    field("r", num(r)),
                    field("g", num(g)),
                    field("b", num(b)),
                ]),
            ),
        ]),
    )]))
}

/// `{ error, message }` for `images.js` to raise.
fn failure(error: PixelsError) -> Value {
    Value::Object(vec![
        (
            "error".to_string(),
            Value::String(error.code().as_str().to_string()),
        ),
        ("message".to_string(), Value::String(error.to_string())),
    ])
}

// ---- arguments --------------------------------------------------------------

/// `(source, open, steps)`, the first three arguments of every image op.
fn parse_common(
    args: &mut [Value],
    from_file: bool,
) -> std::result::Result<(Source, Open, Vec<Step>), OpError> {
    let source = if from_file {
        Source::File(string_arg(args, 0, "source")?)
    } else {
        let bytes = args
            .get_mut(0)
            .map(|v| std::mem::replace(v, Value::Undefined))
            .and_then(|v| match v {
                Value::Bytes(bytes) => Some(bytes),
                _ => None,
            })
            .ok_or_else(|| OpError::type_error("an image source must be bytes"))?;
        Source::Bytes(bytes)
    };
    let open = parse_open(args.get(1))?;
    let steps = match args.get(2) {
        Some(Value::Array(steps)) => steps
            .iter()
            .map(parse_step)
            .collect::<std::result::Result<_, _>>()?,
        _ => Vec::new(),
    };
    Ok((source, open, steps))
}

fn parse_open(value: Option<&Value>) -> std::result::Result<Open, OpError> {
    let mut open = Open::default();
    let Some(Value::Object(fields)) = value else {
        return Ok(open);
    };
    let options = &mut open.options;
    for (key, value) in fields {
        match (key.as_str(), value) {
            ("maxPixels", Value::Number(n)) => {
                open.limits = Limits::default().with_max_pixels(*n as u64);
                *options = options.with_limits(open.limits);
            }
            ("autoOrient", Value::Bool(b)) => *options = options.with_auto_orient(*b),
            ("toSrgb", Value::Bool(b)) => *options = options.with_to_srgb(*b),
            ("animated", Value::Bool(b)) => *options = options.with_animated(*b),
            ("page", Value::Number(n)) => {
                open.page = n.clamp(0.0, f64::from(u32::MAX)) as u32;
            }
            ("pages", Value::Number(n)) => {
                open.pages = Some(if *n < 0.0 {
                    None
                } else {
                    Some(n.clamp(1.0, f64::from(u32::MAX)) as u32)
                });
            }
            // A deadline past what a `Duration` holds is no deadline.
            ("timeout", Value::Number(ms)) if *ms > 0.0 => {
                open.timeout = Duration::try_from_secs_f64(ms / 1000.0).ok();
            }
            ("raw", Value::Object(raw)) => open.decode = Decode::Raw(parse_layout(raw, "raw")?),
            ("create", Value::Object(create)) => {
                let layout = parse_layout(create, "create")?;
                let fill = create
                    .iter()
                    .find(|(k, _)| k == "background")
                    .and_then(|(_, v)| rgba(Some(v)))
                    .unwrap_or([0, 0, 0, 255]);
                open.decode = Decode::Create { layout, fill };
            }
            _ => {}
        }
    }
    Ok(open)
}

/// `{ width, height, channels, depth }` as a layout: 1 to 4 channels
/// (grey, grey and alpha, RGB, RGBA) of sharp's `uchar`, `ushort` or `float`.
fn parse_layout(fields: &[(String, Value)], what: &str) -> std::result::Result<Layout, OpError> {
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    let dim = |key: &str| -> std::result::Result<u32, OpError> {
        match get(key).and_then(Value::as_number) {
            Some(n) if n >= 1.0 && n <= f64::from(u32::MAX) && n.fract() == 0.0 => Ok(n as u32),
            _ => Err(OpError::range_error(format!(
                "{what}.{key} must be a positive integer"
            ))),
        }
    };
    let (width, height) = (dim("width")?, dim("height")?);
    let layout = match get("channels").and_then(Value::as_number) {
        Some(1.0) => ChannelLayout::Gray,
        Some(2.0) => ChannelLayout::GrayAlpha,
        Some(3.0) => ChannelLayout::Rgb,
        Some(4.0) => ChannelLayout::Rgba,
        _ => {
            return Err(OpError::range_error(format!(
                "{what}.channels must be 1, 2, 3 or 4"
            )));
        }
    };
    let kind = parse_depth(get("depth").and_then(Value::as_str).unwrap_or("uchar"))?;
    let pixel = PixelFormat::from_parts(layout, kind).ok_or_else(|| {
        OpError::range_error(format!(
            "{what}: {} channels of {kind:?} is not a pixel format Pixels has",
            layout.channels()
        ))
    })?;
    Ok(Layout {
        width,
        height,
        pixel,
    })
}

/// sharp's names for the sample types Pixels has.
fn parse_depth(name: &str) -> std::result::Result<SampleKind, OpError> {
    Ok(match name {
        "uchar" => SampleKind::U8,
        "ushort" => SampleKind::U16,
        "float" => SampleKind::F32,
        other => {
            return Err(OpError::type_error(format!(
                "unknown depth {other:?}: uchar, ushort or float"
            )));
        }
    })
}

fn parse_step(value: &Value) -> std::result::Result<Step, OpError> {
    let Value::Array(parts) = value else {
        return Err(OpError::type_error("an image step must be an array"));
    };
    let name = parts.first().and_then(Value::as_str).unwrap_or("");
    let num = |i: usize| -> f64 { parts.get(i).and_then(Value::as_number).unwrap_or(0.0) };
    let int = |i: usize| -> u32 { num(i).clamp(0.0, f64::from(u32::MAX)) as u32 };
    Ok(match name {
        "resize" => {
            let fit = match parts.get(3).and_then(Value::as_str).unwrap_or("fill") {
                "fill" => Fit::Fill,
                "inside" => Fit::Inside,
                "outside" => Fit::Outside,
                "cover" => Fit::Cover,
                "contain" => Fit::Contain,
                other => return Err(OpError::type_error(format!("unknown fit {other:?}"))),
            };
            let filter = parse_filter(parts.get(4).and_then(Value::as_str).unwrap_or("lanczos3"))?;
            let background = rgba(parts.get(5)).unwrap_or([0, 0, 0, 0]);
            let without_enlargement = matches!(parts.get(6), Some(Value::Bool(true)));
            let position =
                parse_position(parts.get(7).and_then(Value::as_str).unwrap_or("center"))?;
            let without_reduction = matches!(parts.get(8), Some(Value::Bool(true)));
            Step::Resize {
                width: int(1),
                height: int(2),
                options: ResizeOptions::default()
                    .with_fit(fit)
                    .with_filter(filter)
                    .with_background(background)
                    .without_enlargement(without_enlargement)
                    .without_reduction(without_reduction)
                    .with_position(position),
            }
        }
        "crop" => Step::Crop {
            left: int(1),
            top: int(2),
            width: int(3),
            height: int(4),
        },
        "rotate" => {
            let degrees = num(1);
            if !degrees.is_finite() {
                return Err(OpError::range_error("rotate takes a finite angle"));
            }
            Step::Rotate {
                degrees,
                background: rgba(parts.get(2)).unwrap_or([0, 0, 0, 255]),
            }
        }
        "affine" => {
            let matrix = match parts.get(1) {
                Some(Value::Array(items)) if items.len() == 4 => {
                    let n = |i: usize| items[i].as_number().filter(|n| n.is_finite());
                    match (n(0), n(1), n(2), n(3)) {
                        (Some(a), Some(b), Some(c), Some(d)) => [a, b, c, d],
                        _ => return Err(OpError::type_error("affine takes four finite numbers")),
                    }
                }
                _ => return Err(OpError::type_error("affine takes four finite numbers")),
            };
            let interpolator = match parts.get(7).and_then(Value::as_str).unwrap_or("bicubic") {
                "nearest" => Interpolator::Nearest,
                "bilinear" => Interpolator::Bilinear,
                "bicubic" => Interpolator::Bicubic,
                other => {
                    return Err(OpError::type_error(format!(
                        "unknown interpolator {other:?}: nearest, bilinear or bicubic"
                    )));
                }
            };
            Step::Affine {
                matrix,
                options: AffineOptions::default()
                    .with_background(rgba(parts.get(2)).unwrap_or([0, 0, 0, 255]))
                    .with_input_offset(num(3), num(4))
                    .with_output_offset(num(5), num(6))
                    .with_interpolator(interpolator),
            }
        }
        "frame" => Step::Frame(int(1)),
        "stackFrames" => Step::StackFrames,
        "flip" => Step::Flip,
        "flop" => Step::Flop,
        "modulate" => Step::Modulate {
            brightness: num(1) as f32,
            saturation: num(2) as f32,
            hue: num(3) as f32,
            lightness: num(4) as f32,
        },
        "blur" => Step::Blur(num(1) as f32),
        "sharpen" => Step::Sharpen(num(1) as f32),
        "flatten" => Step::Flatten([num(1) as u8, num(2) as u8, num(3) as u8]),
        "grayscale" => Step::Grayscale,
        "extractChannel" => Step::ExtractChannel(int(1) as usize),
        "extractChannelNamed" => {
            Step::ExtractChannelNamed(match parts.get(1).and_then(Value::as_str).unwrap_or("") {
                "red" => Channel::Red,
                "green" => Channel::Green,
                "blue" => Channel::Blue,
                "alpha" => Channel::Alpha,
                other => {
                    return Err(OpError::type_error(format!(
                        "unknown channel {other:?}: red, green, blue or alpha"
                    )));
                }
            })
        }
        "negate" => Step::Negate {
            alpha: !matches!(parts.get(1), Some(Value::Bool(false))),
        },
        "gamma" => Step::Gamma(num(1) as f32),
        "linear" => {
            let channel = |i: usize| -> [f32; 3] {
                match parts.get(i) {
                    Some(Value::Array(items)) => [
                        items.first().and_then(Value::as_number).unwrap_or(0.0) as f32,
                        items.get(1).and_then(Value::as_number).unwrap_or(0.0) as f32,
                        items.get(2).and_then(Value::as_number).unwrap_or(0.0) as f32,
                    ],
                    _ => [num(i) as f32; 3],
                }
            };
            Step::Linear {
                a: channel(1),
                b: channel(2),
            }
        }
        "threshold" => {
            let level = num(1);
            if !(0.0..=255.0).contains(&level) || level.fract() != 0.0 {
                return Err(OpError::type_error(
                    "threshold level must be an integer from 0 to 255",
                ));
            }
            Step::Threshold {
                level: level as u8,
                greyscale: !matches!(parts.get(2), Some(Value::Bool(false))),
            }
        }
        "tint" => Step::Tint([num(1) as u8, num(2) as u8, num(3) as u8]),
        "greyscale" => Step::Greyscale,
        "recomb" => {
            let size = int(1) as usize;
            if size != 3 && size != 4 {
                return Err(OpError::type_error("recomb takes a 3x3 or 4x4 matrix"));
            }
            let flat: Vec<f32> = parts
                .get(2)
                .and_then(|v| match v {
                    Value::Array(items) => Some(items),
                    _ => None,
                })
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_number)
                        .map(|n| n as f32)
                        .collect()
                })
                .unwrap_or_default();
            if flat.len() != size * size {
                return Err(OpError::type_error("recomb takes a 3x3 or 4x4 matrix"));
            }
            if size == 3 {
                Step::Recomb3([
                    [flat[0], flat[1], flat[2]],
                    [flat[3], flat[4], flat[5]],
                    [flat[6], flat[7], flat[8]],
                ])
            } else {
                Step::Recomb4([
                    [flat[0], flat[1], flat[2], flat[3]],
                    [flat[4], flat[5], flat[6], flat[7]],
                    [flat[8], flat[9], flat[10], flat[11]],
                    [flat[12], flat[13], flat[14], flat[15]],
                ])
            }
        }
        "ensureAlpha" => Step::EnsureAlpha(num(1) as f32),
        "convolve" => {
            let taps: Vec<f32> = match parts.get(3) {
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|v| v.as_number().map(|n| n as f32))
                    .collect::<Option<_>>()
                    .ok_or_else(|| OpError::type_error("a kernel holds numbers"))?,
                _ => return Err(OpError::type_error("convolve takes a kernel")),
            };
            Step::Convolve(
                Kernel::new(int(1), int(2), &taps)
                    .map_err(|e| OpError::range_error(e.to_string()))?,
            )
        }
        "removeAlpha" => Step::RemoveAlpha,
        "extend" => Step::Extend {
            margins: [int(1), int(2), int(3), int(4)],
            with: match parts.get(5).and_then(Value::as_str).unwrap_or("background") {
                "background" => ExtendWith::Background,
                "copy" => ExtendWith::Copy,
                "repeat" => ExtendWith::Repeat,
                "mirror" => ExtendWith::Mirror,
                other => {
                    return Err(OpError::type_error(format!(
                        "unknown extendWith {other:?}: background, copy, repeat or mirror"
                    )));
                }
            },
            background: rgba(parts.get(6)).unwrap_or([0, 0, 0, 255]),
        },
        "trim" => {
            let threshold = parts.get(2).and_then(Value::as_number).unwrap_or(10.0);
            if !threshold.is_finite() || threshold < 0.0 {
                return Err(OpError::range_error("trim threshold must not be negative"));
            }
            Step::Trim {
                background: rgba(parts.get(1)),
                threshold: threshold as f32,
                line_art: matches!(parts.get(3), Some(Value::Bool(true))),
            }
        }
        "composite" => {
            let Some(Value::Array(overlays)) = parts.get(1) else {
                return Err(OpError::type_error("composite takes an array of overlays"));
            };
            Step::Composite(
                overlays
                    .iter()
                    .map(parse_overlay)
                    .collect::<std::result::Result<_, _>>()?,
            )
        }
        other => return Err(OpError::type_error(format!("unknown image step {other:?}"))),
    })
}

/// `{ bytes, open, steps, blend, gravity, left, top, tile }`, as `images.js`
/// resolves a `composite` overlay.
fn parse_overlay(value: &Value) -> std::result::Result<OverlayStep, OpError> {
    let Value::Object(fields) = value else {
        return Err(OpError::type_error("a composite overlay must be an object"));
    };
    let get = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
    let bytes = get("bytes")
        .and_then(Value::as_bytes)
        .ok_or_else(|| OpError::type_error("a composite overlay must be bytes"))?
        .to_vec();
    let steps = match get("steps") {
        Some(Value::Array(steps)) => steps
            .iter()
            .map(parse_step)
            .collect::<std::result::Result<_, _>>()?,
        _ => Vec::new(),
    };
    let offset = match (
        get("left").and_then(Value::as_number),
        get("top").and_then(Value::as_number),
    ) {
        (Some(left), Some(top)) => Some((left as i64, top as i64)),
        (None, None) => None,
        _ => {
            return Err(OpError::type_error(
                "a composite overlay needs both left and top, or neither",
            ));
        }
    };
    Ok(OverlayStep {
        bytes,
        open: parse_open(get("open"))?,
        steps,
        blend: parse_blend(get("blend").and_then(Value::as_str).unwrap_or("over"))?,
        gravity: parse_position(get("gravity").and_then(Value::as_str).unwrap_or("center"))?,
        offset,
        tile: matches!(get("tile"), Some(Value::Bool(true))),
    })
}

/// `[r, g, b, a]` bytes, or `None` if `value` is not an array.
fn rgba(value: Option<&Value>) -> Option<[u8; 4]> {
    let Some(Value::Array(rgba)) = value else {
        return None;
    };
    let c = |i: usize| rgba.get(i).and_then(Value::as_number).unwrap_or(0.0) as u8;
    Some([c(0), c(1), c(2), c(3)])
}

/// sharp's nine gravities and their aliases.
fn parse_position(name: &str) -> std::result::Result<Position, OpError> {
    Ok(match name {
        "center" | "centre" => Position::Centre,
        "north" | "top" => Position::North,
        "northeast" | "right top" => Position::NorthEast,
        "east" | "right" => Position::East,
        "southeast" | "right bottom" => Position::SouthEast,
        "south" | "bottom" => Position::South,
        "southwest" | "left bottom" => Position::SouthWest,
        "west" | "left" => Position::West,
        "northwest" | "left top" => Position::NorthWest,
        "attention" | "entropy" => {
            return Err(OpError::type_error(
                "position \"attention\" and \"entropy\" need saliency, which is not supported",
            ));
        }
        other => {
            return Err(OpError::type_error(format!(
                "unknown position {other:?}: center, north, northeast, east, \
                 southeast, south, southwest, west or northwest"
            )));
        }
    })
}

/// sharp's 25 blend modes, under its names (both spellings of the colour ones).
fn parse_blend(name: &str) -> std::result::Result<Blend, OpError> {
    Ok(match name {
        "clear" => Blend::Clear,
        "source" => Blend::Source,
        "over" => Blend::Over,
        "in" => Blend::In,
        "out" => Blend::Out,
        "atop" => Blend::Atop,
        "dest" => Blend::Dest,
        "dest-over" => Blend::DestOver,
        "dest-in" => Blend::DestIn,
        "dest-out" => Blend::DestOut,
        "dest-atop" => Blend::DestAtop,
        "xor" => Blend::Xor,
        "add" => Blend::Add,
        "saturate" => Blend::Saturate,
        "multiply" => Blend::Multiply,
        "screen" => Blend::Screen,
        "overlay" => Blend::Overlay,
        "darken" => Blend::Darken,
        "lighten" => Blend::Lighten,
        "colour-dodge" | "color-dodge" => Blend::ColourDodge,
        "colour-burn" | "color-burn" => Blend::ColourBurn,
        "hard-light" => Blend::HardLight,
        "soft-light" => Blend::SoftLight,
        "difference" => Blend::Difference,
        "exclusion" => Blend::Exclusion,
        other => return Err(OpError::type_error(format!("unknown blend {other:?}"))),
    })
}

fn parse_filter(name: &str) -> std::result::Result<Filter, OpError> {
    Ok(match name {
        "nearest" => Filter::Nearest,
        "box" => Filter::Box,
        "bilinear" | "linear" => Filter::Bilinear,
        "catmull-rom" | "cubic" => Filter::CatmullRom,
        "mitchell" => Filter::Mitchell,
        "lanczos2" => Filter::Lanczos2,
        "lanczos3" => Filter::Lanczos3,
        other => {
            return Err(OpError::type_error(format!(
                "unknown resize filter {other:?}"
            )));
        }
    })
}

/// The format method's options and the metadata the chain keeps or sets. The
/// ranges are checked in `images.js`; Pixels' own check backs them here.
fn parse_output(value: Option<&Value>) -> std::result::Result<Output, OpError> {
    let mut output = Output::default();
    let Some(Value::Object(fields)) = value else {
        return Ok(output);
    };
    let invalid = |e: PixelsError| OpError::range_error(e.to_string());
    let byte = |n: f64| n.clamp(0.0, 255.0) as u8;
    // Quality first: it starts the options, and the rest refine them.
    if let Some((_, Value::Number(q))) = fields.iter().find(|(k, _)| k == "quality") {
        output.options = EncodeOptions::with_quality(byte(*q)).map_err(invalid)?;
    }
    let options = &mut output.options;
    for (key, value) in fields {
        match (key.as_str(), value) {
            ("format", Value::String(name)) => {
                output.format = Some(match name.as_str() {
                    "jpeg" => Format::Jpeg,
                    "png" => Format::Png,
                    "webp" => Format::WebP,
                    "avif" => Format::Avif,
                    "gif" => Format::Gif,
                    "tiff" => Format::Tiff,
                    "jxl" => Format::Jxl,
                    "raw" => Format::Raw,
                    other => {
                        return Err(OpError::type_error(format!(
                            "unknown image format {other:?}"
                        )));
                    }
                });
            }
            ("lossless", Value::Bool(b)) => *options = options.with_lossless(*b),
            ("depth", Value::String(name)) => output.depth = Some(parse_depth(name)?),
            ("progressive", Value::Bool(b)) => *options = options.with_progressive(*b),
            ("effort", Value::Number(n)) => {
                *options = options.with_effort(byte(*n)).map_err(invalid)?;
            }
            ("compressionLevel", Value::Number(n)) => {
                *options = options.with_compression_level(byte(*n)).map_err(invalid)?;
            }
            ("palette", Value::Bool(b)) => *options = options.with_palette(*b),
            ("colours", Value::Number(n)) => {
                *options = options
                    .with_colours(n.clamp(0.0, f64::from(u16::MAX)) as u16)
                    .map_err(invalid)?;
            }
            ("dither", Value::Number(n)) => {
                *options = options.with_dither(*n as f32).map_err(invalid)?;
            }
            ("alphaQuality", Value::Number(n)) => {
                *options = options.with_alpha_quality(byte(*n)).map_err(invalid)?;
            }
            ("nearLossless", Value::Bool(b)) => *options = options.with_near_lossless(*b),
            ("chromaSubsampling", Value::String(s)) => {
                *options = options.with_chroma_subsampling(match s.as_str() {
                    "4:2:0" => ChromaSubsampling::Yuv420,
                    "4:4:4" => ChromaSubsampling::Yuv444,
                    other => {
                        return Err(OpError::type_error(format!(
                            "unknown chromaSubsampling {other:?}: \"4:2:0\" or \"4:4:4\""
                        )));
                    }
                });
            }
            ("interFrameMaxError", Value::Number(n)) => {
                *options = options
                    .with_inter_frame_max_error(*n as f32)
                    .map_err(invalid)?;
            }
            ("interPaletteMaxError", Value::Number(n)) => {
                *options = options
                    .with_inter_palette_max_error(*n as f32)
                    .map_err(invalid)?;
            }
            ("keepDuplicateFrames", Value::Bool(b)) => {
                *options = options.with_keep_duplicate_frames(*b);
            }
            ("loop", Value::Number(n)) => {
                output.loop_count = Some(n.clamp(0.0, f64::from(u32::MAX)) as u32);
            }
            ("delay", Value::Number(ms)) => {
                output.delay = Some(Delay::Every(ms.clamp(0.0, f64::from(u32::MAX)) as u32));
            }
            ("delay", Value::Array(each)) => {
                output.delay = Some(Delay::Each(
                    each.iter()
                        .map(|v| {
                            v.as_number()
                                .map(|ms| ms.clamp(0.0, f64::from(u32::MAX)) as u32)
                                .ok_or_else(|| OpError::type_error("a delay is a number"))
                        })
                        .collect::<std::result::Result<_, _>>()?,
                ));
            }
            ("keepExif", Value::Bool(b)) => *options = options.with_keep_exif(*b),
            ("keepXmp", Value::Bool(b)) => *options = options.with_keep_xmp(*b),
            ("xmp", Value::Bytes(bytes)) => output.xmp = Some(bytes.clone()),
            ("density", Value::Number(dpi)) => {
                if !(dpi.is_finite() && *dpi > 0.0) {
                    return Err(OpError::range_error("density must be a positive number"));
                }
                output.density = Some(*dpi);
            }
            _ => {}
        }
    }
    Ok(output)
}

fn string_arg(args: &[Value], index: usize, what: &str) -> std::result::Result<String, OpError> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| OpError::type_error(format!("an image {what} path must be a string")))
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

/// Fuzz entry: arbitrary bytes through the path `images(bytes)` takes — the
/// format sniffed from the leading bytes, the decode, `metadata()`, `stats()`
/// and a re-encode in the source's format (see [`crate::fuzz`]). Pixels fuzzes
/// some of its decoders upstream; this reaches every one of them, and the
/// runtime's own handling of what they return.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_decode(bytes: &[u8]) {
    // Small enough that a header claiming a large canvas is refused before
    // anything is allocated, which keeps every iteration cheap; the refusal at
    // the header is itself what the limit exists for.
    let limits = Limits::default().with_max_pixels(1 << 16);
    // Few frames, so an animation is walked and re-encoded whole every
    // iteration without one input costing the run.
    let limits = limits.with_max_frames(8);
    for animated in [false, true] {
        let open = Open {
            options: OpenOptions::default()
                .with_limits(limits)
                .with_animated(animated),
            limits,
            ..Open::default()
        };
        let Ok(image) = pipeline(bytes.to_vec(), open, Vec::new(), &Cancel::never()) else {
            continue;
        };
        let _ = metadata(&image);
        let _ = stats(&image);
        if let Ok(meta) = image.metadata() {
            let _ = image.output(meta.format, EncodeOptions::default()).bytes();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        bytes: Vec<u8>,
        open: Open,
        steps: Vec<Step>,
    ) -> std::result::Result<Image, PixelsError> {
        pipeline(bytes, open, steps, &Cancel::never())
    }

    fn run_encode(
        bytes: Vec<u8>,
        open: Open,
        steps: Vec<Step>,
        output: Output,
    ) -> std::result::Result<(Vec<u8>, Format), PixelsError> {
        encode(bytes, open, steps, output, &Cancel::never())
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let pixels = vec![200_u8; (width * height * 4) as usize];
        otf_pixels::ImageDescriptor::new(width, height, PixelFormat::Rgba8)
            .and_then(|desc| Image::from_raw(desc, pixels))
            .and_then(|image| image.output(Format::Png, EncodeOptions::default()).bytes())
            .unwrap()
    }

    fn step(parts: Vec<Value>) -> Step {
        parse_step(&Value::Array(parts)).unwrap()
    }

    fn s(text: &str) -> Value {
        Value::String(text.to_string())
    }

    #[test]
    fn keep_aspect_fills_the_missing_side() {
        assert_eq!(keep_aspect(400, 0, 1600, 1200), (400, 300));
        assert_eq!(keep_aspect(0, 300, 1600, 1200), (400, 300));
        // Never collapses a side to zero.
        assert_eq!(keep_aspect(1, 0, 4000, 10), (1, 1));
    }

    #[test]
    fn parses_every_step() {
        assert!(matches!(
            step(vec![
                s("resize"),
                Value::Number(10.0),
                Value::Number(0.0),
                s("cover"),
                s("cubic")
            ]),
            Step::Resize {
                width: 10,
                height: 0,
                ..
            }
        ));
        assert_eq!(
            step(vec![
                s("crop"),
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0)
            ]),
            Step::Crop {
                left: 1,
                top: 2,
                width: 3,
                height: 4
            }
        );
        assert_eq!(
            step(vec![s("rotate"), Value::Number(270.0)]),
            Step::Rotate {
                degrees: 270.0,
                background: [0, 0, 0, 255],
            }
        );
        assert_eq!(step(vec![s("grayscale")]), Step::Grayscale);
        assert_eq!(
            step(vec![
                s("flatten"),
                Value::Number(255.0),
                Value::Number(0.0),
                Value::Number(9.0)
            ]),
            Step::Flatten([255, 0, 9])
        );
    }

    #[test]
    fn refuses_unknown_names() {
        assert!(parse_step(&Value::Array(vec![s("explode")])).is_err());
        assert!(parse_filter("mks2021").is_err());
        let output = Value::Object(vec![("format".to_string(), s("heic"))]);
        assert!(parse_output(Some(&output)).is_err());
    }

    #[test]
    fn parses_resize_position_and_reduction() {
        let options = |step: Step| match step {
            Step::Resize { options, .. } => options,
            _ => panic!(),
        };
        let resize = |position: &str, reduction: bool| {
            let mut parts = vec![
                s("resize"),
                Value::Number(10.0),
                Value::Number(10.0),
                s("cover"),
                s("lanczos3"),
                Value::Array(vec![]),
                Value::Bool(false),
                s(position),
            ];
            if reduction {
                parts.push(Value::Bool(true));
            }
            step(parts)
        };
        assert_eq!(
            options(resize("northwest", false)).position,
            Position::NorthWest
        );
        assert_eq!(
            options(resize("left top", false)).position,
            Position::NorthWest
        );
        assert!(!options(resize("center", false)).without_reduction);
        assert!(options(resize("center", true)).without_reduction);
        assert!(
            parse_step(&Value::Array(vec![
                s("resize"),
                Value::Number(10.0),
                Value::Number(10.0),
                s("cover"),
                s("lanczos3"),
                Value::Array(vec![]),
                Value::Bool(false),
                s("attention"),
            ]))
            .is_err()
        );
    }

    #[test]
    fn parses_the_adjustment_steps() {
        assert!(matches!(
            step(vec![s("negate"), Value::Bool(false)]),
            Step::Negate { alpha: false }
        ));
        assert!(matches!(
            step(vec![s("gamma"), Value::Number(2.2)]),
            Step::Gamma(_)
        ));
        assert!(matches!(
            step(vec![
                s("linear"),
                Value::Array(vec![Value::Number(1.0); 3]),
                Value::Array(vec![Value::Number(0.0); 3]),
            ]),
            Step::Linear { .. }
        ));
        assert!(matches!(
            step(vec![
                s("threshold"),
                Value::Number(128.0),
                Value::Bool(true)
            ]),
            Step::Threshold {
                level: 128,
                greyscale: true
            }
        ));
        assert!(parse_step(&Value::Array(vec![s("threshold"), Value::Number(300.0)])).is_err());
        assert!(matches!(
            step(vec![
                s("tint"),
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ]),
            Step::Tint([1, 2, 3])
        ));
        assert_eq!(step(vec![s("greyscale")]), Step::Greyscale);
        assert!(matches!(
            step(vec![
                s("recomb"),
                Value::Number(3.0),
                Value::Array(vec![Value::Number(1.0); 9]),
            ]),
            Step::Recomb3(_)
        ));
        assert!(parse_step(&Value::Array(vec![s("recomb"), Value::Number(3.0)])).is_err());
        assert!(matches!(
            step(vec![s("ensureAlpha"), Value::Number(0.5)]),
            Step::EnsureAlpha(_)
        ));
        assert_eq!(step(vec![s("removeAlpha")]), Step::RemoveAlpha);
        assert_eq!(
            step(vec![s("extractChannelNamed"), s("green")]),
            Step::ExtractChannelNamed(Channel::Green)
        );
    }

    #[test]
    fn adjustments_apply_without_error() {
        let image = png(4, 4);
        let steps = [
            Step::Negate { alpha: true },
            Step::Gamma(2.2),
            Step::Linear {
                a: [1.0; 3],
                b: [0.0; 3],
            },
            Step::Threshold {
                level: 128,
                greyscale: true,
            },
            Step::Tint([255, 0, 0]),
            Step::Greyscale,
            Step::Recomb3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
            Step::EnsureAlpha(1.0),
            Step::RemoveAlpha,
        ];
        for step in steps {
            let out = run(image.clone(), Open::default(), vec![step.clone()]);
            assert!(out.is_ok(), "{step:?}");
        }
    }

    #[test]
    fn pages_defaults_to_one() {
        let image = run(png(4, 4), Open::default(), vec![]).unwrap();
        assert_eq!(image.pages(), 1);
        let open = parse_open(Some(&Value::Object(vec![(
            "page".to_string(),
            Value::Number(1.0),
        )])))
        .unwrap();
        assert!(run(png(4, 4), open, vec![]).is_err());
    }

    #[test]
    fn resizes_with_one_side_given() {
        let (out, _) = run_encode(
            png(40, 20),
            Open::default(),
            vec![step(vec![
                s("resize"),
                Value::Number(10.0),
                Value::Number(0.0),
            ])],
            Output::default(),
        )
        .unwrap();
        let meta = Image::from_stream(Cursor::new(out))
            .unwrap()
            .metadata()
            .unwrap();
        assert_eq!((meta.width, meta.height, meta.format), (10, 5, Format::Png));
    }

    #[test]
    fn grayscale_keeps_alpha() {
        let image = run(png(4, 4), Open::default(), vec![Step::Grayscale]).unwrap();
        assert_eq!(image.metadata().unwrap().pixel, PixelFormat::GrayA8);
    }

    #[test]
    fn failures_carry_the_stable_code() {
        let error = run_encode(
            b"not an image".to_vec(),
            Open::default(),
            vec![],
            Output::default(),
        )
        .unwrap_err();
        let Value::Object(fields) = failure(error) else {
            panic!()
        };
        assert_eq!(fields[0], ("error".to_string(), s("unsupported")));
    }

    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    #[test]
    fn parses_extend_trim_and_composite() {
        assert_eq!(
            step(vec![
                s("extend"),
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0),
                Value::Number(4.0),
                s("mirror"),
            ]),
            Step::Extend {
                margins: [1, 2, 3, 4],
                with: ExtendWith::Mirror,
                background: [0, 0, 0, 255],
            }
        );
        assert!(
            parse_step(&Value::Array(vec![
                s("extend"),
                Value::Number(1.0),
                Value::Number(1.0),
                Value::Number(1.0),
                Value::Number(1.0),
                s("smear")
            ]))
            .is_err()
        );
        assert_eq!(
            step(vec![
                s("trim"),
                Value::Null,
                Value::Number(20.0),
                Value::Bool(true)
            ]),
            Step::Trim {
                background: None,
                threshold: 20.0,
                line_art: true,
            }
        );
        assert!(
            parse_step(&Value::Array(vec![
                s("trim"),
                Value::Null,
                Value::Number(-1.0)
            ]))
            .is_err()
        );

        let overlay = |extra: Vec<(&str, Value)>| {
            let mut fields = vec![("bytes", Value::Bytes(png(2, 2)))];
            fields.extend(extra);
            parse_step(&Value::Array(vec![
                s("composite"),
                Value::Array(vec![object(fields)]),
            ]))
        };
        let Step::Composite(overlays) = overlay(vec![
            ("blend", s("color-dodge")),
            ("gravity", s("southeast")),
            ("left", Value::Number(-3.0)),
            ("top", Value::Number(4.0)),
            ("tile", Value::Bool(true)),
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(overlays[0].blend, Blend::ColourDodge);
        assert_eq!(overlays[0].gravity, Position::SouthEast);
        assert_eq!(overlays[0].offset, Some((-3, 4)));
        assert!(overlays[0].tile);
        assert!(overlay(vec![("blend", s("burn"))]).is_err());
        assert!(overlay(vec![("left", Value::Number(1.0))]).is_err());
        assert!(
            parse_step(&Value::Array(vec![
                s("composite"),
                Value::Array(vec![object(vec![])])
            ]))
            .is_err()
        );
    }

    #[test]
    fn parses_encoder_options() {
        let output = parse_output(Some(&object(vec![
            ("format", s("jxl")),
            ("quality", Value::Number(70.0)),
            ("progressive", Value::Bool(true)),
            ("effort", Value::Number(2.0)),
            ("compressionLevel", Value::Number(9.0)),
            ("colours", Value::Number(16.0)),
            ("dither", Value::Number(0.5)),
            ("alphaQuality", Value::Number(40.0)),
            ("nearLossless", Value::Bool(true)),
            ("chromaSubsampling", s("4:4:4")),
            ("keepExif", Value::Bool(true)),
            ("keepXmp", Value::Bool(true)),
            ("xmp", Value::Bytes(b"<x/>".to_vec())),
            ("density", Value::Number(300.0)),
        ])))
        .unwrap();
        let o = output.options;
        assert_eq!(output.format, Some(Format::Jxl));
        assert_eq!(o.quality, 70);
        assert!(o.progressive && o.near_lossless && o.keep_exif && o.keep_xmp);
        assert_eq!(
            (
                o.effort,
                o.compression_level,
                o.colours,
                o.dither,
                o.alpha_quality
            ),
            (Some(2), Some(9), Some(16), Some(0.5), 40)
        );
        assert_eq!(o.chroma_subsampling, Some(ChromaSubsampling::Yuv444));
        assert_eq!(output.xmp.as_deref(), Some(&b"<x/>"[..]));
        assert_eq!(output.density, Some(300.0));
        for bad in [
            ("compressionLevel", Value::Number(10.0)),
            ("colours", Value::Number(1.0)),
            ("dither", Value::Number(2.0)),
            ("chromaSubsampling", s("4:2:2")),
            ("density", Value::Number(0.0)),
        ] {
            assert!(
                parse_output(Some(&object(vec![bad.clone()]))).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn extend_trim_and_composite_apply() {
        let image = run(
            png(4, 4),
            Open::default(),
            vec![Step::Extend {
                margins: [1, 2, 3, 4],
                with: ExtendWith::Background,
                background: [9, 9, 9, 255],
            }],
        )
        .unwrap();
        assert_eq!(
            (
                image.metadata().unwrap().width,
                image.metadata().unwrap().height
            ),
            (10, 8)
        );
        let bordered = run_encode(
            png(4, 4),
            Open::default(),
            vec![Step::Extend {
                margins: [2, 2, 2, 2],
                with: ExtendWith::Background,
                background: [0, 0, 0, 255],
            }],
            Output::default(),
        )
        .unwrap()
        .0;
        let trimmed = run(
            bordered,
            Open::default(),
            vec![Step::Trim {
                background: None,
                threshold: 10.0,
                line_art: false,
            }],
        )
        .unwrap();
        assert_eq!(trimmed.metadata().unwrap().width, 4);
        assert_eq!(trimmed.trim_offset(), Some((2, 2)));
        let composed = run(
            png(8, 8),
            Open::default(),
            vec![Step::Composite(vec![OverlayStep {
                bytes: png(2, 2),
                open: Open::default(),
                steps: vec![Step::Flip],
                blend: Blend::Multiply,
                gravity: Position::Centre,
                offset: Some((1, 1)),
                tile: false,
            }])],
        )
        .unwrap();
        assert_eq!(composed.metadata().unwrap().width, 8);
    }

    #[test]
    fn metadata_and_stats_report_sharp_fields() {
        let image = run(png(4, 4), Open::default(), vec![]).unwrap();
        let Value::Object(fields) = metadata(&image).unwrap() else {
            panic!()
        };
        let Value::Object(meta) = &fields[0].1 else {
            panic!()
        };
        let keys: Vec<&str> = meta.iter().map(|(k, _)| k.as_str()).collect();
        for key in [
            "channels",
            "orientation",
            "isProgressive",
            "isPalette",
            "hasProfile",
        ] {
            assert!(keys.contains(&key), "{key}");
        }
        assert!(!keys.contains(&"density") && !keys.contains(&"trimOffsetLeft"));

        let Value::Object(fields) = stats(&image).unwrap() else {
            panic!()
        };
        let Value::Object(stats) = &fields[0].1 else {
            panic!()
        };
        let Some((_, Value::Array(channels))) = stats.iter().find(|(k, _)| k == "channels") else {
            panic!()
        };
        assert_eq!(channels.len(), 4);
        // The helper fills alpha with 200 too, so the image is not opaque.
        assert!(stats.contains(&("isOpaque".to_string(), Value::Bool(false))));
    }

    #[test]
    fn raw_and_created_sources_open_and_round_trip() {
        let raw = |w: f64, h: f64, c: f64, depth: &str| {
            object(vec![(
                "raw",
                object(vec![
                    ("width", Value::Number(w)),
                    ("height", Value::Number(h)),
                    ("channels", Value::Number(c)),
                    ("depth", s(depth)),
                ]),
            )])
        };
        let open = parse_open(Some(&raw(2.0, 1.0, 3.0, "uchar"))).unwrap();
        let output = Output {
            format: Some(Format::Raw),
            ..Output::default()
        };
        let pixels = vec![1, 2, 3, 4, 5, 6];
        let (out, format) =
            run_encode(pixels.clone(), open.clone(), vec![], output.clone()).unwrap();
        assert_eq!((out, format), (pixels, Format::Raw));
        // Widened on the way out: two bytes a sample.
        let wide = Output {
            depth: Some(SampleKind::U16),
            ..output
        };
        assert_eq!(
            run_encode(vec![9; 6], open, vec![], wide).unwrap().0.len(),
            12
        );
        assert!(parse_open(Some(&raw(2.0, 1.0, 5.0, "uchar"))).is_err());
        assert!(parse_open(Some(&raw(0.0, 1.0, 1.0, "uchar"))).is_err());
        assert!(parse_open(Some(&raw(2.0, 1.0, 1.0, "double"))).is_err());
        // Grey and alpha at 16 bits is not a pixel format Pixels has.
        assert!(parse_open(Some(&raw(2.0, 1.0, 2.0, "ushort"))).is_err());

        let create = parse_open(Some(&object(vec![(
            "create",
            object(vec![
                ("width", Value::Number(2.0)),
                ("height", Value::Number(2.0)),
                ("channels", Value::Number(4.0)),
                (
                    "background",
                    Value::Array(vec![
                        Value::Number(1.0),
                        Value::Number(2.0),
                        Value::Number(3.0),
                        Value::Number(4.0),
                    ]),
                ),
            ]),
        )])))
        .unwrap();
        let canvas = run_encode(
            Vec::new(),
            create,
            vec![],
            Output {
                format: Some(Format::Raw),
                ..Output::default()
            },
        )
        .unwrap()
        .0;
        assert_eq!(canvas, [1, 2, 3, 4].repeat(4));

        // The limit applies to a raw source, as it does at a file's header.
        let mut limited = parse_open(Some(&raw(2.0, 2.0, 1.0, "uchar"))).unwrap();
        limited.limits = Limits::default().with_max_pixels(3);
        let error = run(vec![0; 4], limited, vec![]).unwrap_err();
        assert_eq!(error.code().as_str(), "limit_exceeded");
    }

    #[test]
    fn parses_convolve() {
        let convolve = |w: f64, h: f64, taps: Vec<f64>| {
            parse_step(&Value::Array(vec![
                s("convolve"),
                Value::Number(w),
                Value::Number(h),
                Value::Array(taps.into_iter().map(Value::Number).collect()),
            ]))
        };
        assert!(matches!(
            convolve(3.0, 1.0, vec![1.0, 2.0, 1.0]),
            Ok(Step::Convolve(_))
        ));
        assert!(convolve(2.0, 1.0, vec![1.0, 1.0]).is_err());
        assert!(convolve(3.0, 3.0, vec![1.0; 4]).is_err());
        let image = run(
            png(4, 4),
            Open::default(),
            vec![convolve(3.0, 3.0, vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]).unwrap()],
        )
        .unwrap();
        assert_eq!(image.metadata().unwrap().width, 4);
    }

    #[test]
    fn max_pixels_refuses_at_the_header() {
        let open = parse_open(Some(&Value::Object(vec![(
            "maxPixels".to_string(),
            Value::Number(100.0),
        )])))
        .unwrap();
        let error = run(png(20, 20), open, vec![])
            .and_then(|i| i.metadata())
            .unwrap_err();
        assert_eq!(error.code().as_str(), "limit_exceeded");
    }

    /// Two 8x8 frames, 100 and 200 ms, looping forever.
    const ANIMATED_GIF: &[u8] =
        include_bytes!("../../runtime-cli/tests/fixtures/images/animated.gif");

    fn animated() -> Open {
        parse_open(Some(&object(vec![("animated", Value::Bool(true))]))).unwrap()
    }

    #[test]
    fn parses_rotate_affine_and_frame_steps() {
        assert_eq!(
            step(vec![
                s("rotate"),
                Value::Number(30.5),
                rgba_value([1, 2, 3, 4])
            ]),
            Step::Rotate {
                degrees: 30.5,
                background: [1, 2, 3, 4],
            }
        );
        let Step::Affine { matrix, options } = step(vec![
            s("affine"),
            Value::Array(
                vec![2.0, 0.5, 0.0, 1.0]
                    .into_iter()
                    .map(Value::Number)
                    .collect(),
            ),
            Value::Null,
            Value::Number(1.0),
            Value::Number(2.0),
            Value::Number(3.0),
            Value::Number(4.0),
            s("nearest"),
        ]) else {
            panic!("not affine")
        };
        assert_eq!(matrix, [2.0, 0.5, 0.0, 1.0]);
        assert_eq!(options.background(), [0, 0, 0, 255]);
        assert_eq!(options.input_offset(), (1.0, 2.0));
        assert_eq!(options.output_offset(), (3.0, 4.0));
        assert_eq!(options.interpolator(), Interpolator::Nearest);
        assert_eq!(step(vec![s("frame"), Value::Number(3.0)]), Step::Frame(3));
        assert_eq!(step(vec![s("stackFrames")]), Step::StackFrames);

        for bad in [
            vec![s("rotate"), Value::Number(f64::NAN)],
            vec![s("affine"), Value::Array(vec![Value::Number(1.0)])],
            vec![
                s("affine"),
                Value::Array(vec![Value::Number(1.0); 4]),
                Value::Null,
                Value::Number(0.0),
                Value::Number(0.0),
                Value::Number(0.0),
                Value::Number(0.0),
                s("nohalo"),
            ],
        ] {
            assert!(parse_step(&Value::Array(bad)).is_err());
        }
    }

    fn rgba_value(c: [u8; 4]) -> Value {
        Value::Array(c.iter().map(|&b| Value::Number(f64::from(b))).collect())
    }

    #[test]
    fn rotation_by_any_angle_and_affine_apply() {
        let rotated = run(
            png(20, 10),
            Open::default(),
            vec![step(vec![s("rotate"), Value::Number(90.0)])],
        )
        .and_then(|i| i.descriptor())
        .unwrap();
        assert_eq!(
            (rotated.width, rotated.height),
            (10, 20),
            "quarter turn is exact"
        );

        let tilted = run(
            png(20, 20),
            Open::default(),
            vec![step(vec![s("rotate"), Value::Number(45.0)])],
        )
        .and_then(|i| i.descriptor())
        .unwrap();
        assert!(
            tilted.width > 20 && tilted.width <= 29,
            "bounding box: {}",
            tilted.width
        );

        let stretched = run(
            png(20, 10),
            Open::default(),
            vec![step(vec![
                s("affine"),
                Value::Array(
                    vec![2.0, 0.0, 0.0, 1.0]
                        .into_iter()
                        .map(Value::Number)
                        .collect(),
                ),
            ])],
        )
        .and_then(|i| i.descriptor())
        .unwrap();
        assert_eq!((stretched.width, stretched.height), (40, 10));
    }

    #[test]
    fn page_and_pages_go_to_frames_or_a_tiff_page() {
        let parse = |fields: Vec<(&str, Value)>| parse_open(Some(&object(fields))).unwrap();
        let gif = parse(vec![("page", Value::Number(1.0))])
            .options_for(ANIMATED_GIF)
            .unwrap();
        assert_eq!(
            (gif.first_frame, gif.frame_count, gif.animated),
            (1, None, false)
        );

        let all = parse(vec![("pages", Value::Number(-1.0))])
            .options_for(ANIMATED_GIF)
            .unwrap();
        assert_eq!(
            (all.frame_count, all.animated),
            (None, true),
            "-1 is every frame"
        );

        let one = parse(vec![("pages", Value::Number(1.0))])
            .options_for(ANIMATED_GIF)
            .unwrap();
        assert_eq!((one.frame_count, one.animated), (Some(1), false));

        let tiff = b"II*\0\x08\0\0\0";
        let page = parse(vec![("page", Value::Number(2.0))])
            .options_for(tiff)
            .unwrap();
        assert_eq!((page.page, page.first_frame), (2, 0));
        let error = parse(vec![("pages", Value::Number(2.0))])
            .options_for(tiff)
            .unwrap_err();
        assert_eq!(error.code().as_str(), "unsupported");
    }

    #[test]
    fn an_animation_is_processed_and_written_frame_by_frame() {
        let image = run(ANIMATED_GIF.to_vec(), animated(), vec![]).unwrap();
        assert_eq!(image.frame_count(), 2);

        let output = parse_output(Some(&object(vec![
            ("format", s("gif")),
            ("loop", Value::Number(3.0)),
            (
                "delay",
                Value::Array(vec![Value::Number(40.0), Value::Number(60.0)]),
            ),
        ])))
        .unwrap();
        let (bytes, format) = run_encode(
            ANIMATED_GIF.to_vec(),
            animated(),
            vec![step(vec![
                s("resize"),
                Value::Number(4.0),
                Value::Number(0.0),
            ])],
            output,
        )
        .unwrap();
        assert_eq!(format, Format::Gif);
        let back = run(bytes, Open::default(), vec![]).unwrap();
        let animation = back.animation().expect("still animated");
        assert_eq!(animation.frame_count, 2);
        assert_eq!(animation.loop_count, 3);
        assert_eq!(animation.frame_durations_ms, vec![40, 60]);
        assert_eq!(back.descriptor().unwrap().width, 4);

        for format in ["webp", "jxl"] {
            let output = parse_output(Some(&object(vec![("format", s(format))]))).unwrap();
            let (bytes, _) = run_encode(ANIMATED_GIF.to_vec(), animated(), vec![], output).unwrap();
            let back = run(bytes, Open::default(), vec![]).unwrap();
            assert_eq!(back.animation().map(|a| a.frame_count), Some(2), "{format}");
        }
    }

    #[test]
    fn an_animation_becomes_a_still_only_when_asked() {
        let png_out = || parse_output(Some(&object(vec![("format", s("png"))]))).unwrap();
        let error = run_encode(ANIMATED_GIF.to_vec(), animated(), vec![], png_out()).unwrap_err();
        assert_eq!(error.code().as_str(), "unsupported", "no silent strip");

        let (bytes, _) = run_encode(
            ANIMATED_GIF.to_vec(),
            animated(),
            vec![Step::Frame(1)],
            png_out(),
        )
        .unwrap();
        let frame = run(bytes, Open::default(), vec![]).unwrap();
        assert_eq!(frame.descriptor().unwrap().height, 8);

        let (bytes, _) = run_encode(
            ANIMATED_GIF.to_vec(),
            animated(),
            vec![Step::StackFrames],
            png_out(),
        )
        .unwrap();
        let strip = run(bytes, Open::default(), vec![]).unwrap();
        let d = strip.descriptor().unwrap();
        assert_eq!((d.width, d.height), (8, 16));

        let error = run(ANIMATED_GIF.to_vec(), animated(), vec![Step::Frame(2)])
            .and_then(|i| i.descriptor())
            .unwrap_err();
        assert_eq!(error.code().as_str(), "invalid_argument");
    }

    #[test]
    fn timing_a_still_is_an_invalid_argument() {
        let output = parse_output(Some(&object(vec![
            ("format", s("gif")),
            ("loop", Value::Number(1.0)),
        ])))
        .unwrap();
        let error = run_encode(png(4, 4), Open::default(), vec![], output).unwrap_err();
        assert_eq!(error.code().as_str(), "invalid_argument");
    }

    #[test]
    fn parses_animated_output_options() {
        let output = parse_output(Some(&object(vec![
            ("format", s("gif")),
            ("delay", Value::Number(70.0)),
            ("interFrameMaxError", Value::Number(8.0)),
            ("interPaletteMaxError", Value::Number(10.0)),
            ("keepDuplicateFrames", Value::Bool(true)),
        ])))
        .unwrap();
        assert_eq!(output.delay, Some(Delay::Every(70)));
        let expected = EncodeOptions::default()
            .with_inter_frame_max_error(8.0)
            .and_then(|o| o.with_inter_palette_max_error(10.0))
            .unwrap()
            .with_keep_duplicate_frames(true);
        assert_eq!(output.options, expected);
        assert!(
            parse_output(Some(&object(vec![(
                "interFrameMaxError",
                Value::Number(40.0)
            )])))
            .is_err()
        );
        assert!(
            parse_output(Some(&object(vec![(
                "delay",
                Value::Array(vec![s("soon")])
            )])))
            .is_err()
        );
    }

    #[test]
    fn a_call_files_its_cancel_until_it_settles() {
        let cancels: Cancels = Rc::default();
        let (cancel, filed) = file_cancel(&cancels, Some(&Value::Number(7.0)), None).unwrap();
        assert!(!cancel.is_never());
        assert!(cancels.borrow().contains_key(&7));
        assert!(
            file_cancel(&cancels, Some(&Value::Number(7.0)), None).is_err(),
            "an id in use is refused"
        );
        cancels.borrow()[&7].cancel();
        assert!(cancel.is_cancelled(), "the filed flag is the run's");
        drop(filed);
        assert!(cancels.borrow().is_empty(), "removed when the call settles");

        let (never, _) = file_cancel(&cancels, None, None).unwrap();
        assert!(never.is_never(), "no signal and no timeout costs nothing");
        assert!(cancels.borrow().is_empty());
        let (deadline, _) = file_cancel(&cancels, None, Some(Duration::ZERO)).unwrap();
        assert!(deadline.is_cancelled(), "a passed deadline cancels");
        assert!(file_cancel(&cancels, Some(&Value::Number(-1.0)), None).is_err());
    }

    #[test]
    fn a_cancelled_run_fails_as_cancelled() {
        let cancel = Cancel::new();
        cancel.cancel();
        let error = pipeline(png(64, 64), Open::default(), vec![Step::Blur(4.0)], &cancel)
            .and_then(|image| image.output(Format::Png, EncodeOptions::default()).bytes())
            .unwrap_err();
        assert_eq!(error.code().as_str(), "cancelled");

        let open = parse_open(Some(&object(vec![("timeout", Value::Number(250.0))]))).unwrap();
        assert_eq!(open.timeout, Some(Duration::from_millis(250)));
        for unbounded in [f64::INFINITY, 1e300, f64::NAN] {
            let open =
                parse_open(Some(&object(vec![("timeout", Value::Number(unbounded))]))).unwrap();
            assert_eq!(open.timeout, None, "{unbounded}");
        }
    }
}
