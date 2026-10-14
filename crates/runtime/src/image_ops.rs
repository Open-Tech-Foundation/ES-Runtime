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

use std::io::Cursor;
use std::sync::Arc;

use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::{FileSystem, ProviderError, TaskSpawner};
use otf_pixels::{
    Channel, EncodeOptions, Filter, Fit, Format, Image, Limits, Modulate, OpenOptions, PixelFormat,
    PixelsError, Position, ResizeOptions,
};

use crate::Result;

/// Where the encoded bytes come from.
enum Source {
    Bytes(Vec<u8>),
    File(String),
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
    Rotate(i32),
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
}

/// The encode target; `None` keeps the source's format.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Output {
    format: Option<Format>,
    quality: Option<u8>,
    lossless: bool,
}

/// Registers the `runtime:images` ops.
pub(crate) fn install(
    engine: &mut dyn Engine,
    fs: Option<Arc<dyn FileSystem>>,
    spawner: Option<Arc<dyn TaskSpawner>>,
) -> Result<()> {
    // (name, source is a file, destination is a file)
    for (name, from_file) in [("image_metadata", false), ("image_metadata_file", true)] {
        let (fs, spawner) = (fs.clone(), spawner.clone());
        let mut decl = OpDecl::r#async(name, move |mut args| {
            let (fs, spawner) = (fs.clone(), spawner.clone());
            let parsed = parse_common(&mut args, from_file);
            Box::pin(async move {
                let (source, open, steps) = parsed?;
                let bytes = read_source(&fs, source).await?;
                let result = offload(spawner, move || {
                    pipeline(bytes, open, &steps).and_then(|image| metadata(&image))
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
        let (fs, spawner) = (fs.clone(), spawner.clone());
        let mut decl = OpDecl::r#async(name, move |mut args| {
            let (fs, spawner) = (fs.clone(), spawner.clone());
            let parsed = parse_common(&mut args, from_file).and_then(|common| {
                let output = parse_output(args.get(3))?;
                let dest = if to_file {
                    Some(string_arg(&args, 4, "destination")?)
                } else {
                    None
                };
                Ok((common, output, dest))
            });
            Box::pin(async move {
                let ((source, open, steps), output, dest) = parsed?;
                let bytes = read_source(&fs, source).await?;
                let encoded = offload(spawner, move || encode(bytes, open, &steps, output)).await?;
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
    // the work panicked, which Pixels promises malformed input cannot cause.
    rx.await
        .map_err(|_| OpError::new(ExceptionClass::Error, "the image work did not complete"))
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
fn pipeline(
    bytes: Vec<u8>,
    open: OpenOptions,
    steps: &[Step],
) -> std::result::Result<Image, PixelsError> {
    let mut image = Image::from_stream_with(Cursor::new(bytes), open)?;
    for step in steps {
        image = apply(image, step)?;
    }
    Ok(image)
}

fn apply(image: Image, step: &Step) -> std::result::Result<Image, PixelsError> {
    Ok(match *step {
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
        Step::Rotate(degrees) => image.rotate(degrees),
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
    open: OpenOptions,
    steps: &[Step],
    output: Output,
) -> std::result::Result<(Vec<u8>, Format), PixelsError> {
    let image = pipeline(bytes, open, steps)?;
    let format = match output.format {
        Some(format) => format,
        None => image.metadata()?.format,
    };
    let options = match output.quality {
        Some(quality) => EncodeOptions::with_quality(quality)?,
        None => EncodeOptions::default(),
    }
    .with_lossless(output.lossless);
    Ok((image.output(format, options).bytes()?, format))
}

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
    Ok(Value::Object(vec![(
        "ok".to_string(),
        Value::Object(vec![
            ("width".to_string(), Value::Number(f64::from(meta.width))),
            ("height".to_string(), Value::Number(f64::from(meta.height))),
            (
                "format".to_string(),
                Value::String(meta.format.as_str().to_string()),
            ),
            (
                "pixelFormat".to_string(),
                Value::String(meta.pixel.as_str().to_string()),
            ),
            ("hasAlpha".to_string(), Value::Bool(meta.pixel.has_alpha())),
            ("animation".to_string(), animation),
            ("pages".to_string(), Value::Number(f64::from(image.pages()))),
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
) -> std::result::Result<(Source, OpenOptions, Vec<Step>), OpError> {
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

fn parse_open(value: Option<&Value>) -> std::result::Result<OpenOptions, OpError> {
    let mut open = OpenOptions::default();
    let Some(Value::Object(fields)) = value else {
        return Ok(open);
    };
    for (key, value) in fields {
        match (key.as_str(), value) {
            ("maxPixels", Value::Number(n)) => {
                open = open.with_limits(Limits::default().with_max_pixels(*n as u64));
            }
            ("autoOrient", Value::Bool(b)) => open = open.with_auto_orient(*b),
            ("toSrgb", Value::Bool(b)) => open = open.with_to_srgb(*b),
            ("animated", Value::Bool(b)) => open = open.with_animated(*b),
            ("page", Value::Number(n)) => {
                open = open.with_page(n.max(0.0).clamp(0.0, f64::from(u32::MAX)) as u32);
            }
            _ => {}
        }
    }
    Ok(open)
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
            let background = match parts.get(5) {
                Some(Value::Array(rgba)) => {
                    let c = |i: usize| rgba.get(i).and_then(Value::as_number).unwrap_or(0.0) as u8;
                    [c(0), c(1), c(2), c(3)]
                }
                _ => [0, 0, 0, 0],
            };
            let without_enlargement = matches!(parts.get(6), Some(Value::Bool(true)));
            let position = match parts.get(7).and_then(Value::as_str).unwrap_or("center") {
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
            };
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
        "rotate" => Step::Rotate(num(1) as i32),
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
        "removeAlpha" => Step::RemoveAlpha,
        other => return Err(OpError::type_error(format!("unknown image step {other:?}"))),
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

fn parse_output(value: Option<&Value>) -> std::result::Result<Output, OpError> {
    let mut output = Output {
        format: None,
        quality: None,
        lossless: false,
    };
    let Some(Value::Object(fields)) = value else {
        return Ok(output);
    };
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
                    other => {
                        return Err(OpError::type_error(format!(
                            "unknown image format {other:?}"
                        )));
                    }
                });
            }
            ("quality", Value::Number(q)) => output.quality = Some(q.clamp(0.0, 255.0) as u8),
            ("lossless", Value::Bool(b)) => output.lossless = *b,
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

#[cfg(test)]
mod tests {
    use super::*;

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
            Step::Rotate(270)
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
            let out = pipeline(
                image.clone(),
                OpenOptions::default(),
                std::slice::from_ref(&step),
            );
            assert!(out.is_ok(), "{step:?}");
        }
    }

    #[test]
    fn pages_defaults_to_one() {
        let image = pipeline(png(4, 4), OpenOptions::default(), &[]).unwrap();
        assert_eq!(image.pages(), 1);
        let open = parse_open(Some(&Value::Object(vec![(
            "page".to_string(),
            Value::Number(1.0),
        )])))
        .unwrap();
        assert!(pipeline(png(4, 4), open, &[]).is_err());
    }

    #[test]
    fn resizes_with_one_side_given() {
        let (out, _) = encode(
            png(40, 20),
            OpenOptions::default(),
            &[step(vec![
                s("resize"),
                Value::Number(10.0),
                Value::Number(0.0),
            ])],
            Output {
                format: None,
                quality: None,
                lossless: false,
            },
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
        let image = pipeline(png(4, 4), OpenOptions::default(), &[Step::Grayscale]).unwrap();
        assert_eq!(image.metadata().unwrap().pixel, PixelFormat::GrayA8);
    }

    #[test]
    fn failures_carry_the_stable_code() {
        let error = encode(
            b"not an image".to_vec(),
            OpenOptions::default(),
            &[],
            Output {
                format: None,
                quality: None,
                lossless: false,
            },
        )
        .unwrap_err();
        let Value::Object(fields) = failure(error) else {
            panic!()
        };
        assert_eq!(fields[0], ("error".to_string(), s("unsupported")));
    }

    #[test]
    fn max_pixels_refuses_at_the_header() {
        let open = parse_open(Some(&Value::Object(vec![(
            "maxPixels".to_string(),
            Value::Number(100.0),
        )])))
        .unwrap();
        let error = pipeline(png(20, 20), open, &[])
            .and_then(|i| i.metadata())
            .unwrap_err();
        assert_eq!(error.code().as_str(), "limit_exceeded");
    }
}
