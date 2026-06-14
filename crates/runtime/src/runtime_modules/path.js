// runtime:path — modern, platform-aware path utilities (DECISIONS D26, SPEC §11).
//
// Pure computation: it performs no I/O. The host platform and working directory
// come from runtime:process, so `sep`, `resolve()`, and friends follow the real
// OS (importing this module therefore needs the `Env` capability, as evaluating
// runtime:process does). Deliberately free of legacy Node baggage: one
// platform-correct surface — no `posix`/`win32` dual namespaces, no overloaded
// signatures — plus first-class file: URL interop (the modern `__dirname`).

import { platform, cwd } from "runtime:process";

const WINDOWS = platform === "windows";
const sep = WINDOWS ? "\\" : "/";
const delimiter = WINDOWS ? ";" : ":";

// On Windows either slash separates; on POSIX only "/".
const SPLIT = WINDOWS ? /[\\/]+/ : /\/+/;
const isSepChar = (c) => c === "/" || (WINDOWS && c === "\\");
const isDrive = (p) => WINDOWS && p.length >= 2 && p[1] === ":" && /[A-Za-z]/.test(p[0]);

function str(p, name = "path") {
  if (typeof p !== "string") throw new TypeError(`${name} must be a string, got ${typeof p}`);
  return p;
}

function isAbsolute(p) {
  str(p);
  if (WINDOWS) {
    if (isDrive(p)) return p.length >= 3 && isSepChar(p[2]); // "C:\..."
    return p.length >= 1 && isSepChar(p[0]); // "\..." or "/..."
  }
  return p.length >= 1 && p[0] === "/";
}

// Splits a path into its absolute root ("", "/", or "C:\\") and the remainder.
function split(p) {
  if (WINDOWS) {
    if (isDrive(p)) {
      const abs = p.length >= 3 && isSepChar(p[2]);
      return { root: p.slice(0, 2) + (abs ? sep : ""), rest: p.slice(abs ? 3 : 2), abs };
    }
    if (p.length >= 1 && isSepChar(p[0])) return { root: sep, rest: p.slice(1), abs: true };
    return { root: "", rest: p, abs: false };
  }
  if (p.length >= 1 && p[0] === "/") return { root: "/", rest: p.slice(1), abs: true };
  return { root: "", rest: p, abs: false };
}

// Collapses "." and ".." across a list of segments. Leading ".." are kept only
// for relative paths (an absolute path cannot climb above its root).
function collapse(rest, abs) {
  const out = [];
  for (const s of rest.split(SPLIT)) {
    if (s === "" || s === ".") continue;
    if (s === "..") {
      if (out.length && out[out.length - 1] !== "..") out.pop();
      else if (!abs) out.push("..");
    } else {
      out.push(s);
    }
  }
  return out;
}

function normalize(p) {
  str(p);
  if (p.length === 0) return ".";
  const { root, rest, abs } = split(p);
  const body = collapse(rest, abs).join(sep);
  const result = root + body;
  return result.length === 0 ? "." : result;
}

function join(...segments) {
  const parts = segments.filter((s) => str(s, "segment").length > 0);
  if (parts.length === 0) return ".";
  return normalize(parts.join(sep));
}

// Resolves to an absolute path, walking segments right-to-left until one is
// absolute; anything still relative is anchored at the current directory.
function resolve(...segments) {
  let resolved = "";
  let abs = false;
  for (let i = segments.length - 1; i >= 0 && !abs; i--) {
    const s = str(segments[i], "segment");
    if (s.length === 0) continue;
    resolved = resolved.length ? s + sep + resolved : s;
    abs = isAbsolute(s);
  }
  if (!abs) resolved = resolved.length ? cwd() + sep + resolved : cwd();
  return normalize(resolved);
}

function dirname(p) {
  str(p);
  const { root, rest } = split(p);
  const parts = rest.split(SPLIT).filter((s) => s.length > 0);
  if (parts.length <= 1) return root.length ? root.replace(/[\\/]+$/, "") || sep : ".";
  return root + parts.slice(0, -1).join(sep);
}

function basename(p) {
  str(p);
  const parts = split(p).rest.split(SPLIT).filter((s) => s.length > 0);
  return parts.length ? parts[parts.length - 1] : "";
}

function extname(p) {
  const base = basename(p);
  const dot = base.lastIndexOf(".");
  return dot <= 0 ? "" : base.slice(dot);
}

function parse(p) {
  str(p);
  const { root } = split(p);
  const base = basename(p);
  const ext = extname(p);
  return {
    root,
    dir: dirname(p),
    base,
    name: ext ? base.slice(0, -ext.length) : base,
    ext,
  };
}

function relative(from, to) {
  const a = resolve(str(from, "from"));
  const b = resolve(str(to, "to"));
  if (a === b) return "";
  const ap = split(a).rest.split(SPLIT).filter(Boolean);
  const bp = split(b).rest.split(SPLIT).filter(Boolean);
  let i = 0;
  while (i < ap.length && i < bp.length && ap[i] === bp[i]) i++;
  const up = new Array(ap.length - i).fill("..");
  return [...up, ...bp.slice(i)].join(sep) || ".";
}

// file: URL interop — the modern replacement for __dirname:
//   dirname(fromFileURL(import.meta.url))
function fromFileURL(url) {
  const u = url instanceof URL ? url : new URL(str(url, "url"));
  if (u.protocol !== "file:") throw new TypeError(`expected a file: URL, got ${u.protocol}`);
  let p = decodeURIComponent(u.pathname);
  if (WINDOWS) p = p.replace(/^\//, "").replace(/\//g, "\\");
  return p;
}

function toFileURL(p) {
  const abs = resolve(str(p));
  const segs = abs.split(SPLIT);
  // Keep a Windows drive ("C:") verbatim; percent-encode every real segment.
  const encoded = segs
    .map((s, i) => (i === 0 && isDrive(abs) ? s : encodeURIComponent(s)))
    .join("/");
  return new URL("file://" + (WINDOWS ? "/" + encoded : encoded));
}

export {
  sep,
  delimiter,
  isAbsolute,
  normalize,
  join,
  resolve,
  dirname,
  basename,
  extname,
  parse,
  relative,
  fromFileURL,
  toFileURL,
};
export default {
  sep,
  delimiter,
  isAbsolute,
  normalize,
  join,
  resolve,
  dirname,
  basename,
  extname,
  parse,
  relative,
  fromFileURL,
  toFileURL,
};
