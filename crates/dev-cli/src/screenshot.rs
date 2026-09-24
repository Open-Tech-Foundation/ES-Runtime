//! `toMatchScreenshot` in browser runs: comparing an element's pixels with a
//! committed reference image (DECISIONS D120).
//!
//! # PNG, as far as a browser writes it
//!
//! Every screenshot a browser returns over WebDriver BiDi is a PNG of 8-bit
//! RGB or RGBA, not interlaced. That is all [`decode`] reads and all [`encode`]
//! writes, on `flate2`, which the runtime already carries, rather than a new
//! image crate. A reference edited in an image program into something else is
//! refused by name.
//!
//! # The comparison is pixelmatch's
//!
//! [`compare`] is a port of pixelmatch, the comparator Vitest's
//! `toMatchScreenshot` uses by default, so a `threshold` means the same here:
//! the difference between two colours is measured in YIQ, where it follows
//! what the eye sees, and a pixel that differs only because an edge was
//! antialiased differently is not counted.
//!
//! pixelmatch is Copyright (c) 2025, Mapbox, under the ISC License:
//!
//! > Permission to use, copy, modify, and/or distribute this software for any
//! > purpose with or without fee is hereby granted, provided that the above
//! > copyright notice and this permission notice appear in all copies.
//! >
//! > THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
//! > WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
//! > MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
//! > ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
//! > WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
//! > ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR
//! > IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// An image, four bytes a pixel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// Reads a PNG of 8-bit RGB, RGBA, grey or grey with alpha, not interlaced.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    let rest = bytes.strip_prefix(SIGNATURE).ok_or("not a PNG")?;
    let mut at = 0;
    let (mut width, mut height, mut color) = (0u32, 0u32, 0u8);
    let mut data = Vec::new();
    while at + 8 <= rest.len() {
        let length = u32::from_be_bytes(rest[at..at + 4].try_into().unwrap_or_default()) as usize;
        let kind = &rest[at + 4..at + 8];
        let body = rest
            .get(at + 8..at + 8 + length)
            .ok_or("a PNG chunk runs past the end of the file")?;
        match kind {
            b"IHDR" => {
                if body.len() < 13 {
                    return Err("a PNG header is too short".to_string());
                }
                width = u32::from_be_bytes(body[0..4].try_into().unwrap_or_default());
                height = u32::from_be_bytes(body[4..8].try_into().unwrap_or_default());
                let (depth, interlace) = (body[8], body[12]);
                color = body[9];
                if depth != 8 || interlace != 0 || ![0, 2, 4, 6].contains(&color) {
                    return Err(format!(
                        "only 8-bit, non-interlaced grey, RGB and RGBA PNGs are read \
                         (this one: bit depth {depth}, colour type {color}, interlace {interlace})"
                    ));
                }
            }
            b"IDAT" => data.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + length;
    }
    if width == 0 || height == 0 {
        return Err("a PNG with no header or no pixels".to_string());
    }
    let channels = match color {
        0 => 1,
        4 => 2,
        2 => 3,
        _ => 4,
    };
    let stride = width as usize * channels;
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&data[..])
        .read_to_end(&mut raw)
        .map_err(|err| format!("a PNG's pixels do not inflate: {err}"))?;
    if raw.len() < (stride + 1) * height as usize {
        return Err("a PNG has fewer rows than its header says".to_string());
    }
    let mut pixels = vec![0u8; stride * height as usize];
    for row in 0..height as usize {
        let filter = raw[row * (stride + 1)];
        let line = &raw[row * (stride + 1) + 1..(row + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels {
                pixels[row * stride + x - channels]
            } else {
                0
            };
            let b = if row > 0 {
                pixels[(row - 1) * stride + x]
            } else {
                0
            };
            let c = if row > 0 && x >= channels {
                pixels[(row - 1) * stride + x - channels]
            } else {
                0
            };
            let predicted = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => paeth(a, b, c),
                other => return Err(format!("a PNG row has an unknown filter {other}")),
            };
            pixels[row * stride + x] = line[x].wrapping_add(predicted);
        }
    }
    let rgba = match channels {
        4 => pixels,
        _ => pixels
            .chunks(channels)
            .flat_map(|px| match px {
                [g] => [*g, *g, *g, 255],
                [g, a] => [*g, *g, *g, *a],
                [r, g, b] => [*r, *g, *b, 255],
                _ => [0, 0, 0, 0],
            })
            .collect(),
    };
    Ok(Image {
        width,
        height,
        rgba,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = i16::from(a) + i16::from(b) - i16::from(c);
    let (pa, pb, pc) = (
        (p - i16::from(a)).abs(),
        (p - i16::from(b)).abs(),
        (p - i16::from(c)).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Writes an RGBA PNG.
pub fn encode(image: &Image) -> Vec<u8> {
    let stride = image.width as usize * 4;
    // Each row after the first filtered by the one above: images of pages are
    // mostly runs of one colour, which this turns into runs of zeros.
    let mut unfiltered_above: Option<&[u8]> = None;
    let mut filtered = Vec::with_capacity((stride + 1) * image.height as usize);
    for row in image.rgba.chunks(stride) {
        match unfiltered_above {
            None => {
                filtered.push(0);
                filtered.extend_from_slice(row);
            }
            Some(above) => {
                filtered.push(2);
                filtered.extend(row.iter().zip(above).map(|(x, up)| x.wrapping_sub(*up)));
            }
        }
        unfiltered_above = Some(row);
    }
    let mut deflater = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = deflater.write_all(&filtered);
    let data = deflater.finish().unwrap_or_default();

    let mut out = SIGNATURE.to_vec();
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&image.width.to_be_bytes());
    header.extend_from_slice(&image.height.to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &data);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(u32::MAX).to_be_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(kind);
    crc.update(body);
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out.extend_from_slice(&crc.finalize().to_be_bytes());
}

/// How different two images may be, as Vitest's pixelmatch options say it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// How different two colours may be and still be the same pixel, 0 to 1.
    pub threshold: f64,
    /// How many pixels may differ.
    pub allowed_pixels: Option<u64>,
    /// What share of the pixels may differ, 0 to 1.
    pub allowed_ratio: Option<f64>,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance {
            threshold: 0.1,
            allowed_pixels: None,
            allowed_ratio: None,
        }
    }
}

impl Tolerance {
    /// Whether this many mismatched pixels, of `total`, are allowed.
    pub fn allows(&self, mismatched: u64, total: u64) -> bool {
        if let Some(pixels) = self.allowed_pixels {
            return mismatched <= pixels;
        }
        if let Some(ratio) = self.allowed_ratio {
            #[expect(
                clippy::cast_precision_loss,
                reason = "pixel counts are far below 2^52"
            )]
            return total > 0 && (mismatched as f64 / total as f64) <= ratio;
        }
        mismatched == 0
    }
}

/// How many pixels differ, and an image showing where: the reference faded,
/// red where they differ, yellow where only antialiasing does.
pub fn compare(expected: &Image, actual: &Image, threshold: f64) -> (u64, Image) {
    let (width, height) = (expected.width as usize, expected.height as usize);
    let max_delta = 35215.0 * threshold * threshold;
    let mut diff = vec![0u8; width * height * 4];
    let mut mismatched = 0u64;
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let delta = color_delta(&expected.rgba, &actual.rgba, at, at, false);
            let out = if delta.abs() > max_delta {
                if antialiased(expected, x, y, actual) || antialiased(actual, x, y, expected) {
                    [255, 255, 0, 255]
                } else {
                    mismatched += 1;
                    [255, 0, 0, 255]
                }
            } else {
                let grey = blend(gray_of(&expected.rgba, at), 0.1);
                [grey, grey, grey, 255]
            };
            diff[at..at + 4].copy_from_slice(&out);
        }
    }
    (
        mismatched,
        Image {
            width: expected.width,
            height: expected.height,
            rgba: diff,
        },
    )
}

fn gray_of(rgba: &[u8], at: usize) -> f64 {
    let a = f64::from(rgba[at + 3]) / 255.0;
    let (r, g, b) = (
        blend(f64::from(rgba[at]), a),
        blend(f64::from(rgba[at + 1]), a),
        blend(f64::from(rgba[at + 2]), a),
    );
    f64::from(r) * 0.298_895_31 + f64::from(g) * 0.586_622_47 + f64::from(b) * 0.114_482_23
}

/// A channel over white, at alpha `a`.
fn blend(channel: f64, a: f64) -> u8 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "0 to 255"
    )]
    let blended = (255.0 + (channel - 255.0) * a).round().clamp(0.0, 255.0) as u8;
    blended
}

/// pixelmatch's `colorDelta`: the squared YIQ distance, or with `y_only` the
/// brightness difference, signed by which is lighter.
fn color_delta(a: &[u8], b: &[u8], k: usize, m: usize, y_only: bool) -> f64 {
    let channels = |data: &[u8], at: usize| {
        let alpha = f64::from(data[at + 3]);
        let over_white = |c: u8| {
            if alpha < 255.0 {
                255.0 + (f64::from(c) - 255.0) * alpha / 255.0
            } else {
                f64::from(c)
            }
        };
        (
            over_white(data[at]),
            over_white(data[at + 1]),
            over_white(data[at + 2]),
        )
    };
    let (r1, g1, b1) = channels(a, k);
    let (r2, g2, b2) = channels(b, m);
    if (r1, g1, b1) == (r2, g2, b2) {
        return 0.0;
    }
    let y1 = r1 * 0.298_895_31 + g1 * 0.586_622_47 + b1 * 0.114_482_23;
    let y2 = r2 * 0.298_895_31 + g2 * 0.586_622_47 + b2 * 0.114_482_23;
    let y = y1 - y2;
    if y_only {
        return y;
    }
    let i = (r1 * 0.595_977_99 - g1 * 0.274_176_1 - b1 * 0.321_801_89)
        - (r2 * 0.595_977_99 - g2 * 0.274_176_1 - b2 * 0.321_801_89);
    let q = (r1 * 0.211_470_17 - g1 * 0.522_617_18 + b1 * 0.311_146_94)
        - (r2 * 0.211_470_17 - g2 * 0.522_617_18 + b2 * 0.311_146_94);
    let delta = 0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q;
    if y1 > y2 { -delta } else { delta }
}

/// pixelmatch's `antialiased`: whether the pixel at `(x, y)` of `image` looks
/// like the edge of antialiasing rather than a change.
fn antialiased(image: &Image, x: usize, y: usize, other: &Image) -> bool {
    let (width, height) = (image.width as usize, image.height as usize);
    let (x0, y0) = (x.saturating_sub(1), y.saturating_sub(1));
    let (x2, y2) = ((x + 1).min(width - 1), (y + 1).min(height - 1));
    let at = (y * width + x) * 4;
    let mut zeroes = usize::from(x == x0 || x == x2 || y == y0 || y == y2);
    let (mut min, mut max) = (0.0f64, 0.0f64);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (0, 0, 0, 0);
    for nx in x0..=x2 {
        for ny in y0..=y2 {
            if nx == x && ny == y {
                continue;
            }
            let delta = color_delta(&image.rgba, &image.rgba, at, (ny * width + nx) * 4, true);
            if delta == 0.0 {
                zeroes += 1;
                if zeroes > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                (min_x, min_y) = (nx, ny);
            } else if delta > max {
                max = delta;
                (max_x, max_y) = (nx, ny);
            }
        }
    }
    if min == 0.0 || max == 0.0 {
        return false;
    }
    (has_many_siblings(image, min_x, min_y) && has_many_siblings(other, min_x, min_y))
        || (has_many_siblings(image, max_x, max_y) && has_many_siblings(other, max_x, max_y))
}

/// Whether a pixel has more than two neighbours of exactly its colour.
fn has_many_siblings(image: &Image, x: usize, y: usize) -> bool {
    let (width, height) = (image.width as usize, image.height as usize);
    let (x0, y0) = (x.saturating_sub(1), y.saturating_sub(1));
    let (x2, y2) = ((x + 1).min(width - 1), (y + 1).min(height - 1));
    let at = (y * width + x) * 4;
    let mut zeroes = usize::from(x == x0 || x == x2 || y == y0 || y == y2);
    for nx in x0..=x2 {
        for ny in y0..=y2 {
            if nx == x && ny == y {
                continue;
            }
            let other = (ny * width + nx) * 4;
            if image.rgba[at..at + 4] == image.rgba[other..other + 4] {
                zeroes += 1;
            }
            if zeroes > 2 {
                return true;
            }
        }
    }
    false
}

/// The platform as Vitest names it in a reference's file name, so references
/// written by either are told apart the same way.
pub fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// A name as it can be part of a file name: anything but letters, digits,
/// `.`, `_` and `-` becomes `-`, and runs of them one.
pub fn file_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

/// Where a test file's reference screenshot of `name` is kept:
/// `__screenshots__/<file>/<name>-<browser>-<platform>.png` beside it.
pub fn reference_path(test_file: &Path, name: &str, browser: &str) -> PathBuf {
    let dir = test_file.parent().unwrap_or(Path::new("."));
    let file = test_file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    dir.join("__screenshots__").join(file).join(format!(
        "{}-{browser}-{}.png",
        file_name(name),
        platform()
    ))
}

/// Where a mismatch's actual and diff images are written, under
/// `.esdev/screenshots/` in the project: output to look at, not to commit.
pub fn attachment_paths(root: &Path, reference: &Path) -> (PathBuf, PathBuf) {
    let relative = reference.strip_prefix(root).unwrap_or(reference);
    let base = root.join(".esdev").join("screenshots").join(relative);
    (
        base.with_extension("actual.png"),
        base.with_extension("diff.png"),
    )
}

/// What checking a screenshot against its reference came to: `None` when it
/// passed, or the failure's message.
pub struct Check<'a> {
    pub root: &'a Path,
    pub reference: &'a Path,
    pub update: bool,
    pub ci: bool,
    pub tolerance: Tolerance,
}

impl Check<'_> {
    pub fn run(&self, actual_png: &[u8]) -> Option<String> {
        let shown = self
            .reference
            .strip_prefix(self.root)
            .unwrap_or(self.reference)
            .display()
            .to_string();
        let write_reference = || -> Result<(), String> {
            if let Some(dir) = self.reference.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
            }
            std::fs::write(self.reference, actual_png)
                .map_err(|err| format!("cannot write {shown}: {err}"))
        };
        let Ok(stored) = std::fs::read(self.reference) else {
            if self.ci {
                return Some(format!(
                    "no reference screenshot at {shown}; --ci does not write them"
                ));
            }
            if let Err(err) = write_reference() {
                return Some(err);
            }
            // Written, and still a failure unless asked for: an image nobody
            // has looked at is not yet what the element should look like.
            return (!self.update).then(|| {
                format!(
                    "no reference screenshot was found; one was written to {shown}. \
                     Review it, then run the tests again."
                )
            });
        };
        let expected = match decode(&stored) {
            Ok(image) => image,
            Err(err) => return Some(format!("{shown}: {err}")),
        };
        let actual = match decode(actual_png) {
            Ok(image) => image,
            Err(err) => return Some(format!("the screenshot taken: {err}")),
        };
        let message = if (expected.width, expected.height) != (actual.width, actual.height) {
            Some(format!(
                "screenshot is {}×{}, and {shown} is {}×{}",
                actual.width, actual.height, expected.width, expected.height
            ))
        } else {
            let (mismatched, diff) = compare(&expected, &actual, self.tolerance.threshold);
            let total = u64::from(expected.width) * u64::from(expected.height);
            if self.tolerance.allows(mismatched, total) {
                None
            } else {
                let (actual_path, diff_path) = attachment_paths(self.root, self.reference);
                let written =
                    write_attachments(self.root, &actual_path, actual_png, &diff_path, &diff);
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "pixel counts are far below 2^52"
                )]
                let percent = mismatched as f64 * 100.0 / total as f64;
                Some(format!(
                    "screenshot differs from {shown}: {mismatched} of {total} pixels \
                     ({percent:.2}%){}",
                    written.map_or_else(
                        |err| format!("\n  {err}"),
                        |()| format!(
                            "\n  actual: {}\n  diff:   {}",
                            relative(self.root, &actual_path),
                            relative(self.root, &diff_path)
                        )
                    )
                ))
            }
        };
        match message {
            Some(_) if self.update => write_reference().err(),
            message => message,
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Writes a mismatch's images, with a `.gitignore` in `.esdev/` that keeps the
/// whole directory out of a commit.
fn write_attachments(
    root: &Path,
    actual_path: &Path,
    actual: &[u8],
    diff_path: &Path,
    diff: &Image,
) -> Result<(), String> {
    let esdev = root.join(".esdev");
    if let Some(dir) = actual_path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    }
    let ignore = esdev.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "*\n");
    }
    std::fs::write(actual_path, actual)
        .map_err(|err| format!("cannot write {}: {err}", actual_path.display()))?;
    std::fs::write(diff_path, encode(diff))
        .map_err(|err| format!("cannot write {}: {err}", diff_path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Image {
        Image {
            width,
            height,
            rgba: rgba.repeat((width * height) as usize),
        }
    }

    #[test]
    fn an_image_survives_encoding_and_decoding() {
        let mut image = solid(7, 5, [10, 20, 30, 255]);
        for (i, byte) in image.rgba.iter_mut().enumerate() {
            *byte = byte.wrapping_add((i * 37 % 251) as u8);
        }
        let png = encode(&image);
        assert!(png.starts_with(SIGNATURE));
        assert_eq!(decode(&png).unwrap(), image);
    }

    #[test]
    fn every_row_filter_decodes() {
        // Hand-built: a 2×2 RGB image, one row with each of two filters, and
        // a paeth-filtered row, decoded to what they predict.
        let rows: Vec<u8> = vec![
            1, 10, 20, 30, 5, 5, 5, // sub: second pixel is first + 5
            4, 1, 1, 1, 0, 0, 0, // paeth
        ];
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&rows).unwrap();
        let mut png = SIGNATURE.to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&2u32.to_be_bytes());
        header.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        chunk(&mut png, b"IDAT", &z.finish().unwrap());
        chunk(&mut png, b"IEND", &[]);
        let image = decode(&png).unwrap();
        assert_eq!(
            image.rgba,
            [
                10, 20, 30, 255, 15, 25, 35, 255, // row 0
                11, 21, 31, 255, 15, 25, 35, 255, // row 1: paeth picks above, then above
            ]
        );
    }

    #[test]
    fn other_pngs_are_refused_by_what_they_are() {
        assert_eq!(decode(b"GIF89a").unwrap_err(), "not a PNG");
        let mut png = SIGNATURE.to_vec();
        let mut header = Vec::new();
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&1u32.to_be_bytes());
        header.extend_from_slice(&[16, 6, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &header);
        assert!(decode(&png).unwrap_err().contains("bit depth 16"));
    }

    #[test]
    fn identical_images_match_and_a_changed_block_does_not() {
        let a = solid(20, 20, [255, 255, 255, 255]);
        assert_eq!(compare(&a, &a, 0.1).0, 0);
        let mut b = a.clone();
        for y in 5..10 {
            for x in 5..10 {
                let at = (y * 20 + x) * 4;
                b.rgba[at..at + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        let (mismatched, diff) = compare(&a, &b, 0.1);
        assert_eq!(mismatched, 25);
        let at = (7 * 20 + 7) * 4;
        assert_eq!(&diff.rgba[at..at + 4], &[255, 0, 0, 255]);
    }

    #[test]
    fn a_near_colour_is_within_the_threshold() {
        let a = solid(4, 4, [200, 200, 200, 255]);
        let b = solid(4, 4, [203, 201, 200, 255]);
        assert_eq!(compare(&a, &b, 0.1).0, 0);
        assert_eq!(compare(&a, &b, 0.0).0, 16);
    }

    #[test]
    fn tolerances_count_pixels_or_their_share() {
        let default = Tolerance::default();
        assert!(default.allows(0, 100) && !default.allows(1, 100));
        let pixels = Tolerance {
            allowed_pixels: Some(3),
            ..default
        };
        assert!(pixels.allows(3, 100) && !pixels.allows(4, 100));
        let ratio = Tolerance {
            allowed_ratio: Some(0.05),
            ..default
        };
        assert!(ratio.allows(5, 100) && !ratio.allows(6, 100));
    }

    #[test]
    fn references_are_named_as_vitest_names_them() {
        let path = reference_path(
            Path::new("/p/src/button.test.ts"),
            "button > hover 1",
            "firefox",
        );
        assert_eq!(
            path,
            PathBuf::from(format!(
                "/p/src/__screenshots__/button.test.ts/button-hover-1-firefox-{}.png",
                platform()
            ))
        );
        let (actual, diff) = attachment_paths(Path::new("/p"), &path);
        assert!(actual.starts_with("/p/.esdev/screenshots/src/__screenshots__"));
        assert!(actual.to_string_lossy().ends_with(".actual.png"));
        assert!(diff.to_string_lossy().ends_with(".diff.png"));
    }
}
