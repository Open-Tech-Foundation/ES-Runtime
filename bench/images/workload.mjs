// The image workloads both runtimes run. Each is JOBS pipelines started at
// once, so the number is throughput under concurrency, which is how a server
// meets them. Each prints JOBS times the pixel count of its first output as
// a checksum the runner compares, so a runtime cannot look fast by producing
// smaller images, then that output's size in bytes. Only one output is read
// back: reading an image's metadata is not what is being measured.
export const JOBS = 40;

export const WORKLOADS = {
  // A phone-sized JPEG upload to a WebP thumbnail: the common case.
  jpeg_webp: { input: "photo.jpg", width: 400, format: "webp" },
  // The same to a JPEG thumbnail, which isolates the WebP encoder.
  jpeg_jpeg: { input: "photo.jpg", width: 400, format: "jpeg" },
  // A screenshot-like PNG to a JPEG thumbnail, which isolates PNG decoding.
  png_jpeg: { input: "screen.png", width: 320, format: "jpeg" },
  // The same JPEG to an AVIF thumbnail. Bun has no AVIF encoder on Linux.
  jpeg_avif: { input: "photo.jpg", width: 400, format: "avif" },
};
