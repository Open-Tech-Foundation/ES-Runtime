//! Web APIs on the global scope: text, URL, events, streams, blobs, compression.

use super::*;

#[test]
fn performance_reads_the_clock_provider() {
    let _g = v8_guard();
    let mut rt = runtime_with(
        Arc::new(TestConsole::default()),
        Arc::new(FixedClock {
            monotonic: 1234,
            wall: 5678,
        }),
    );
    assert_eq!(rt.eval("performance.now()").unwrap(), Value::Number(1234.0));
    assert_eq!(
        rt.eval("performance.timeOrigin").unwrap(),
        Value::Number(5678.0)
    );
}

/// `navigator.userAgent` reports the crate's own version. The conformance
/// suite can only check the *shape* of the string; this is what stops the
/// substituted version drifting from the binary doing the reporting.
#[test]
fn navigator_user_agent_carries_the_crate_version() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_eq!(
        rt.eval("navigator.userAgent").unwrap(),
        Value::String(concat!("ES-Runtime/", env!("CARGO_PKG_VERSION")).into())
    );
    // The placeholder must be substituted everywhere, not just here.
    assert!(
        !prelude::source().contains("__ES_RUNTIME_VERSION__"),
        "an unsubstituted version token is left in the prelude"
    );
}

#[test]
fn self_aliases_global_this() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_eq!(rt.eval("self === globalThis").unwrap(), Value::Bool(true));
}

#[test]
fn text_encoder_decoder_round_trip() {
    let _g = v8_guard();
    let mut rt = runtime();
    // "héllo😀": é is 2 UTF-8 bytes, 😀 is 4 → 1+2+1+1+1+4 = 10 bytes.
    assert_eq!(
        rt.eval(r#"new TextEncoder().encode("héllo😀").length"#)
            .unwrap(),
        Value::Number(10.0)
    );
    assert_true(
        &mut rt,
        r#"new TextDecoder().decode(new TextEncoder().encode("héllo😀")) === "héllo😀""#,
    );
}

#[test]
fn atob_btoa_round_trip() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(&mut rt, r#"btoa("hello") === "aGVsbG8=""#);
    assert_true(&mut rt, r#"atob("aGVsbG8=") === "hello""#);
    // One character per byte, even when the bytes are valid UTF-8.
    assert_true(
        &mut rt,
        r#"(() => { const s = atob(btoa("zo\u00c3\u00ab")); return s.length === 4 && s.charCodeAt(3) === 0xab; })()"#,
    );
    assert_true(
        &mut rt,
        r#"(() => { try { btoa("Ā"); return false; } catch (e) { return e.name === "InvalidCharacterError"; } })()"#,
    );
}

#[test]
fn structured_clone_rejects_functions() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { try { structuredClone(() => {}); return false; } \
         catch (e) { return e.name === 'DataCloneError'; } })()",
    );
}

#[test]
fn dom_exception_is_an_error_with_name_and_code() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const e = new DOMException('x', 'AbortError'); \
         return e instanceof Error && e.name === 'AbortError' && e.message === 'x' \
         && new DOMException('', 'DataCloneError').code === 25; })()",
    );
}

#[test]
fn url_parses_and_exposes_components() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const u = new URL('https://user:pw@example.com:8080/a/b?x=1&y=2#frag'); \
         return u.protocol === 'https:' && u.hostname === 'example.com' && u.port === '8080' \
         && u.pathname === '/a/b' && u.search === '?x=1&y=2' && u.hash === '#frag' \
         && u.username === 'user' && u.origin === 'https://example.com:8080'; })()",
    );
}

#[test]
fn url_resolves_against_a_base_and_rejects_invalid() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "new URL('../c', 'https://example.com/a/b').href === 'https://example.com/c'",
    );
    assert_true(&mut rt, "URL.canParse('https://ok.test/') === true");
    assert_true(&mut rt, "URL.canParse('not a url') === false");
    assert_true(
        &mut rt,
        "(() => { try { new URL('not a url'); return false; } catch (e) { return e instanceof TypeError; } })()",
    );
}

/// The host keeps a small cache of parsed URLs so a component setter need not
/// re-parse the href it was just handed. `href -> Url` is a pure function, so
/// a hit must be indistinguishable from a miss — this drives enough distinct
/// URLs through it to force eviction, interleaves two objects so neither
/// keeps the other's entry warm, and checks every result against the same
/// value reached by a fresh `new URL()`.
#[test]
fn url_setters_agree_whether_or_not_the_parse_was_cached() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { \
           const a = new URL('https://a.test/p?q=1#f'); \
           const b = new URL('http://b.test:81/x'); \
           for (let i = 0; i < 40; i++) { \
             a.hostname = 'h' + i + '.test'; \
             b.pathname = '/p' + i; \
             b.host = 'k' + i + '.test:' + (8000 + i); \
             a.search = '?n=' + i; \
             if (a.href !== new URL('https://h' + i + '.test/p?n=' + i + '#f').href) return false; \
             if (b.href !== new URL('http://k' + i + '.test:' + (8000 + i) + '/p' + i).href) return false; \
             if (a.origin !== 'https://h' + i + '.test') return false; \
           } \
           return true; })()",
    );
}

/// An invalid component assignment is a silent no-op per WHATWG, and must
/// leave the cache holding the URL it did *not* change — a stale entry here
/// would make the next setter act on a URL the guest never had.
#[test]
fn a_rejected_url_setter_leaves_the_next_one_correct() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { \
           const u = new URL('https://ok.test/a'); \
           u.hostname = 'bad host'; \
           if (u.href !== 'https://ok.test/a') return false; \
           u.port = 'notaport'; \
           if (u.href !== 'https://ok.test/a') return false; \
           u.pathname = '/b'; \
           return u.href === 'https://ok.test/b'; })()",
    );
}

#[test]
fn url_search_params_stay_in_sync() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const u = new URL('https://h.test/?a=1'); \
         u.searchParams.append('b', '2'); \
         return u.search === '?a=1&b=2' && u.searchParams.get('a') === '1' \
         && u.searchParams.getAll('b').length === 1; })()",
    );
    assert_true(
        &mut rt,
        "new URLSearchParams('x=1&x=2&y=3').getAll('x').join(',') === '1,2'",
    );
}

#[test]
fn event_target_dispatches_to_listeners() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const t = new EventTarget(); let got = null; \
         t.addEventListener('x', (e) => { got = e.detail; }); \
         t.dispatchEvent(new CustomEvent('x', { detail: 42 })); return got === 42; })()",
    );
    // once: fires at most once.
    assert_true(
        &mut rt,
        "(() => { const t = new EventTarget(); let n = 0; \
         t.addEventListener('x', () => n++, { once: true }); \
         t.dispatchEvent(new Event('x')); t.dispatchEvent(new Event('x')); return n === 1; })()",
    );
    // preventDefault on a cancelable event → dispatchEvent returns false.
    assert_true(
        &mut rt,
        "(() => { const t = new EventTarget(); t.addEventListener('x', (e) => e.preventDefault()); \
         return t.dispatchEvent(new Event('x', { cancelable: true })) === false; })()",
    );
}

#[test]
fn abort_controller_signals_abort() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const c = new AbortController(); let reason = null; \
         c.signal.addEventListener('abort', () => { reason = c.signal.reason; }); \
         c.abort('stop'); return c.signal.aborted === true && reason === 'stop'; })()",
    );
    // Default reason is an AbortError DOMException.
    assert_true(
        &mut rt,
        "(() => { const c = new AbortController(); c.abort(); \
         return c.signal.reason instanceof DOMException && c.signal.reason.name === 'AbortError'; })()",
    );
    // AbortSignal.any follows the first source to abort.
    assert_true(
        &mut rt,
        "(() => { const a = new AbortController(); const b = new AbortController(); \
         const any = AbortSignal.any([a.signal, b.signal]); let fired = false; \
         any.addEventListener('abort', () => { fired = true; }); \
         b.abort('z'); return any.aborted && any.reason === 'z' && fired; })()",
    );
    // throwIfAborted throws the reason.
    assert_true(
        &mut rt,
        "(() => { try { AbortSignal.abort('e').throwIfAborted(); return false; } \
         catch (err) { return err === 'e'; } })()",
    );
}

#[test]
fn abort_signal_timeout_fires_on_tick() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.eval(
        "globalThis.timedOut = false; \
         const s = AbortSignal.timeout(50); \
         s.addEventListener('abort', () => { globalThis.timedOut = true; });",
    )
    .unwrap();
    // Not yet due.
    assert_eq!(rt.tick(10).timers_fired, 0);
    assert_eq!(rt.eval("globalThis.timedOut").unwrap(), Value::Bool(false));
    // Past the deadline: the timer fires and aborts the signal.
    assert_eq!(rt.tick(50).timers_fired, 1);
    assert_true(&mut rt, "globalThis.timedOut === true");
    assert_true(
        &mut rt,
        "AbortSignal.timeout(0), true", // smoke: constructor path works with 0
    );
}

#[test]
fn readable_stream_reads_enqueued_chunks() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const rs = new ReadableStream({ start(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } }); \
         const r = rs.getReader(); const got = []; let x; \
         while (!(x = await r.read()).done) got.push(x.value); \
         return got.join(',');",
    );
    assert_eq!(out, Value::String("a,b".into()));
}

#[test]
fn readable_stream_pull_drives_the_source() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "let i = 0; \
         const rs = new ReadableStream({ pull(c) { c.enqueue(i++); if (i === 3) c.close(); } }); \
         const r = rs.getReader(); const got = []; let x; \
         while (!(x = await r.read()).done) got.push(x.value); \
         return got.join(',');",
    );
    assert_eq!(out, Value::String("0,1,2".into()));
}

#[test]
fn readable_stream_cancel_calls_the_source() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "let cancelled = null; \
         const rs = new ReadableStream({ cancel(reason) { cancelled = reason; } }); \
         await rs.getReader().cancel('stop'); return cancelled;",
    );
    assert_eq!(out, Value::String("stop".into()));
}

#[test]
fn readable_stream_tee_duplicates_chunks() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const rs = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } }); \
         const [a, b] = rs.tee(); \
         const drain = async (s) => { const r = s.getReader(); const o = []; let x; \
           while (!(x = await r.read()).done) o.push(x.value); return o.join(','); }; \
         const [sa, sb] = await Promise.all([drain(a), drain(b)]); \
         return sa + '|' + sb;",
    );
    assert_eq!(out, Value::String("1,2|1,2".into()));
}

#[test]
fn count_queuing_strategy_reports_backpressure() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const rs = new ReadableStream( \
           { start(c) { globalThis.a = c.desiredSize; c.enqueue('x'); globalThis.b = c.desiredSize; } }, \
           new CountQueuingStrategy({ highWaterMark: 2 })); \
         await Promise.resolve(); return globalThis.a + ',' + globalThis.b;",
    );
    assert_eq!(out, Value::String("2,1".into()));
}

#[test]
fn writable_stream_writes_and_closes() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const written = []; \
         const ws = new WritableStream({ write(chunk) { written.push(chunk); } }); \
         const w = ws.getWriter(); \
         await w.write('a'); await w.write('b'); await w.close(); \
         return written.join(',');",
    );
    assert_eq!(out, Value::String("a,b".into()));
}

#[test]
fn transform_stream_maps_chunks() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const ts = new TransformStream({ transform(chunk, c) { c.enqueue(chunk * 2); } }); \
         const w = ts.writable.getWriter(); const r = ts.readable.getReader(); \
         w.write(1); w.write(2); w.close(); \
         const got = []; let x; \
         while (!(x = await r.read()).done) got.push(x.value); \
         return got.join(',');",
    );
    assert_eq!(out, Value::String("2,4".into()));
}

#[test]
fn pipe_to_moves_chunks_to_the_sink() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const src = new ReadableStream({ start(c) { c.enqueue('x'); c.enqueue('y'); c.close(); } }); \
         const sink = []; \
         const dest = new WritableStream({ write(chunk) { sink.push(chunk); } }); \
         await src.pipeTo(dest); return sink.join(',');",
    );
    assert_eq!(out, Value::String("x,y".into()));
}

#[test]
fn pipe_through_a_transform() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const src = new ReadableStream({ start(c) { c.enqueue(1); c.enqueue(2); c.close(); } }); \
         const ts = new TransformStream({ transform(chunk, c) { c.enqueue(chunk + 10); } }); \
         const r = src.pipeThrough(ts).getReader(); \
         const got = []; let x; \
         while (!(x = await r.read()).done) got.push(x.value); \
         return got.join(',');",
    );
    assert_eq!(out, Value::String("11,12".into()));
}

#[test]
fn text_encoder_stream_round_trips() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const tes = new TextEncoderStream(); \
         const w = tes.writable.getWriter(); const r = tes.readable.getReader(); \
         w.write('hé'); w.write('llo'); w.close(); \
         const bytes = []; let x; \
         while (!(x = await r.read()).done) bytes.push(...x.value); \
         return new TextDecoder().decode(new Uint8Array(bytes));",
    );
    assert_eq!(out, Value::String("héllo".into()));
}

#[test]
fn text_decoder_stream_handles_split_multibyte() {
    let _g = v8_guard();
    let mut rt = runtime();
    // "é" is 0xC3 0xA9, split across two chunks.
    let out = eval_async(
        &mut rt,
        "const tds = new TextDecoderStream(); \
         const w = tds.writable.getWriter(); const r = tds.readable.getReader(); \
         w.write(new Uint8Array([0x68, 0xC3])); \
         w.write(new Uint8Array([0xA9, 0x6F])); \
         w.close(); \
         let s = ''; let x; \
         while (!(x = await r.read()).done) s += x.value; \
         return s;",
    );
    assert_eq!(out, Value::String("héo".into()));
}

#[test]
fn headers_are_case_insensitive_and_combine() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const h = new Headers(); h.append('X-A', '1'); h.append('x-a', '2'); \
         return h.get('X-A') === '1, 2' && h.has('x-a') && !h.has('y'); })()",
    );
}

#[test]
fn blob_concatenates_and_reads() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const b = new Blob(['hello', ' ', 'world'], { type: 'text/plain' }); \
         return b.size + '|' + b.type + '|' + (await b.text());",
    );
    assert_eq!(out, Value::String("11|text/plain|hello world".into()));
}

#[test]
fn form_data_basic_operations() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const f = new FormData(); f.append('a', '1'); f.append('b', '2'); f.append('a', '3'); \
         return f.get('a') === '1' && f.getAll('a').join(',') === '1,3' && f.has('b'); })()",
    );
}

#[test]
fn compression_round_trips_all_formats_across_chunks() {
    let _g = v8_guard();
    let mut rt = runtime();
    // ~64 KiB of compressible text written in 1 KiB chunks, through a
    // CompressionStream piped into a DecompressionStream, per format.
    let out = eval_async(
        &mut rt,
        "const text = 'the quick brown fox jumps over the lazy dog. '.repeat(1500); \
         const bytes = new TextEncoder().encode(text); \
         const results = []; \
         for (const format of ['brotli', 'gzip', 'deflate', 'deflate-raw']) { \
           const pipeline = new ReadableStream({ \
             start(c) { \
               for (let i = 0; i < bytes.length; i += 1024) c.enqueue(bytes.slice(i, i + 1024)); \
               c.close(); \
             }, \
           }) \
             .pipeThrough(new CompressionStream(format)) \
             .pipeThrough(new DecompressionStream(format)); \
           let size = 0; \
           const parts = []; \
           for await (const chunk of pipeline) { parts.push(chunk); size += chunk.length; } \
           const merged = new Uint8Array(size); \
           let at = 0; \
           for (const p of parts) { merged.set(p, at); at += p.length; } \
           results.push(format + ':' + (new TextDecoder().decode(merged) === text)); \
         } \
         return results.join('|');",
    );
    assert_eq!(
        out,
        Value::String("brotli:true|gzip:true|deflate:true|deflate-raw:true".into())
    );
}

#[test]
fn decompression_decodes_known_gzip_and_brotli_vectors() {
    let _g = v8_guard();
    let mut rt = runtime();
    // gzip: `printf 'hello world' | gzip -n | base64`; brotli: Node's
    // `zlib.brotliCompressSync('hello world')` (Google's C encoder) —
    // real-encoder interop for both.
    let out = eval_async(
        &mut rt,
        "const vectors = [ \
           ['gzip', 'H4sIAAAAAAAAA8tIzcnJVyjPL8pJAQCFEUoNCwAAAA=='], \
           ['brotli', 'CwWAaGVsbG8gd29ybGQD'], \
         ]; \
         const results = []; \
         for (const [format, b64] of vectors) { \
           const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)); \
           const ds = new DecompressionStream(format); \
           const writer = ds.writable.getWriter(); \
           writer.write(bytes); \
           writer.close(); \
           const parts = []; \
           for await (const chunk of ds.readable) parts.push(...chunk); \
           results.push(format + ':' + new TextDecoder().decode(new Uint8Array(parts))); \
         } \
         return results.join('|');",
    );
    assert_eq!(
        out,
        Value::String("gzip:hello world|brotli:hello world".into())
    );
}

#[test]
fn compression_stream_validates_format_and_chunk_type() {
    let _g = v8_guard();
    let mut rt = runtime();
    // The chunk type is checked by the transform algorithm, which the sink
    // write only reaches once the readable side pulls, so a reader must be
    // pending for the write to run (and reject) at all.
    let out = eval_async(
        &mut rt,
        "const log = []; \
         try { new CompressionStream('br'); } catch (e) { log.push('c:' + e.name); } \
         try { new DecompressionStream('zip'); } catch (e) { log.push('d:' + e.name); } \
         try { new CompressionStream(); } catch (e) { log.push('none:' + e.name); } \
         const cs = new CompressionStream('gzip'); \
         const w = cs.writable.getWriter(); \
         cs.readable.getReader().read().catch(() => {}); \
         try { await w.write('not bytes'); } catch (e) { log.push('chunk:' + e.name); } \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String("c:TypeError|d:TypeError|none:TypeError|chunk:TypeError".into())
    );
}

#[test]
fn decompression_errors_on_corrupt_and_truncated_input() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const log = []; \
         { const ds = new DecompressionStream('gzip'); \
           const w = ds.writable.getWriter(); \
           const reads = ds.readable.getReader().read().catch((e) => log.push('corrupt-read:' + e.name)); \
           try { await w.write(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8])); await w.close(); } \
           catch (e) { log.push('corrupt:' + e.name); } \
           await reads; } \
         { const ds = new DecompressionStream('deflate'); \
           const w = ds.writable.getWriter(); \
           ds.readable.getReader().read().catch(() => {}); \
           try { await w.write(new Uint8Array([0x78, 0x9c])); await w.close(); } \
           catch (e) { log.push('truncated:' + e.name); } } \
         { const ds = new DecompressionStream('brotli'); \
           const w = ds.writable.getWriter(); \
           ds.readable.getReader().read().catch(() => {}); \
           try { await w.write(new Uint8Array([0x0b, 0x05])); await w.close(); } \
           catch (e) { log.push('br-truncated:' + e.name); } } \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String(
            "corrupt-read:TypeError|corrupt:TypeError|truncated:TypeError|br-truncated:TypeError"
                .into()
        )
    );
}

#[test]
fn transform_stream_cancel_hook_runs_on_abort_and_cancel() {
    let _g = v8_guard();
    let mut rt = runtime();
    // The transformer.cancel hook (Streams spec) fires exactly once on a
    // writable abort or a readable cancel — CompressionStream relies on it
    // to free its native context.
    let out = eval_async(
        &mut rt,
        "const log = []; \
         { const ts = new TransformStream({ cancel(r) { log.push('abort:' + r); } }); \
           await ts.writable.abort('w'); } \
         { const ts = new TransformStream({ cancel(r) { log.push('cancel:' + r); } }); \
           await ts.readable.cancel('r'); } \
         return log.join('|');",
    );
    assert_eq!(out, Value::String("abort:w|cancel:r".into()));
}

/// The in-JS harness the suite is written against — `test`/`todo`, the
/// `assert*` helpers, and the `__results` tally.
///
/// Read from the same file `conformance/run.js` loads, so the CI gate and
/// the `esrun` runner cannot drift apart.
const CONFORMANCE_HARNESS: &str = include_str!("../../conformance/harness.js");

/// Suite files skipped when collecting `conformance/*.js`: the harness
/// itself (loading it again would reset the tally mid-run) and the `esrun`
/// runner (a module, and it would recurse).
const CONFORMANCE_NON_SUITE: &[&str] = &["harness.js", "run.js"];

/// Runs every `conformance/*.js` spec-assertion file and records the
/// pass-rate (SPEC §5 / §8). Gated on zero failures and a non-regressing
/// count; the recorded snapshot lives in `conformance/RESULTS.md`.
#[test]
#[allow(clippy::print_stdout)] // reports the pass-rate under `--nocapture`
fn conformance_suite_passes() {
    let _g = v8_guard();
    // An in-memory filesystem and a full capability set, so files exercising
    // `runtime:fs` are gated here rather than needing a real disk (and
    // leaving artifacts in the working tree).
    let engine = V8Engine::new(Limits::default()).expect("engine");
    let mut rt = Runtime::new(
        Box::new(engine),
        test_providers().with_file_system(Arc::new(MemoryFs::default())),
    )
    .expect("runtime");
    rt.set_capabilities(CapabilitySet::all());
    rt.eval(CONFORMANCE_HARNESS).expect("conformance harness");

    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/conformance");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("read conformance dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| CONFORMANCE_NON_SUITE.contains(&n))
        })
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no conformance files found in {dir}");

    for path in &files {
        let src = std::fs::read_to_string(path).expect("read conformance file");
        rt.eval(&src)
            .unwrap_or_else(|e| panic!("loading {}: {e}", path.display()));
    }

    // Settle the async tests, then read the tallies.
    //
    // Several of them `await import("runtime:…")`. The engine raises that as
    // a *pending dynamic import* — host work the runtime resolves in the
    // async step, not a microtask — so ticking alone can never settle it.
    // That is why four files (every `runtime:serialization` assertion, plus
    // the `runtime:fs` pipeline) used to contribute nothing at all: the old
    // wait gave up after a fixed tick count and read the tallies anyway, so
    // they were silently uncounted rather than failing.
    rt.eval(
        "globalThis.__settled = false; \
         globalThis.__await_all().then(() => { globalThis.__settled = true; }, \
                                       () => { globalThis.__settled = true; });",
    )
    .expect("start settling the async assertions");
    pump_until(&mut rt, "the async conformance assertions", |rt| {
        block_on(rt.process_dynamic_imports()).expect("process dynamic imports");
        rt.eval("globalThis.__settled").unwrap() == Value::Bool(true)
    });

    let number = |rt: &mut Runtime, expr: &str| match rt.eval(expr).unwrap() {
        Value::Number(n) => n as u32,
        other => panic!("{expr} was not a number: {other:?}"),
    };
    let pass = number(&mut rt, "__results.pass");
    let fail = number(&mut rt, "__results.fail");
    let todo = number(&mut rt, "__results.todo");
    let failures = match rt.eval("__results.failures.join('\\n')").unwrap() {
        Value::String(s) => s,
        _ => String::new(),
    };
    let fixed = match rt.eval("__results.fixed.join('\\n')").unwrap() {
        Value::String(s) => s,
        _ => String::new(),
    };

    assert_eq!(fail, 0, "conformance failures ({fail}):\n{failures}");
    // A `todo` that passes means the deviation is fixed: promote it to
    // `test` (and bump BASELINE) so the behaviour is locked in.
    assert!(
        fixed.is_empty(),
        "these `todo` cases now pass — promote them to `test`:\n{fixed}"
    );
    // Non-regression floor; bump alongside conformance/RESULTS.md as the
    // suite grows so removed/skipped assertions are caught.
    const BASELINE: u32 = 430;

    assert!(
        pass >= BASELINE,
        "conformance pass count {pass} below baseline {BASELINE}"
    );
    println!(
        "conformance: {pass}/{} assertions passing across {} files \
         ({todo} known deviations)",
        pass + fail,
        files.len()
    );
}
