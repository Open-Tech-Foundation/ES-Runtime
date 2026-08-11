//! End-to-end ES module tests: run the real `esrun` binary against fixture
//! `.mjs` files (so the actual `FsModuleLoader` + real filesystem + process
//! exit codes are exercised, which the in-process runtime tests — using an
//! in-memory loader — do not). `CARGO_BIN_EXE_esrun` is set by Cargo and points
//! at the freshly built binary.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A `Command` for the built `esrun` binary.
fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

/// Absolute path to a file under `tests/fixtures/`.
fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn run_file(rel: &str) -> Output {
    esrun()
        .arg(fixture(rel))
        .output()
        .expect("failed to spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn runs_a_module_with_imports_meta_and_tla() {
    let out = run_file("main.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    // import.meta.url is a file: URL ending in the entry's path.
    assert!(stdout.contains("URL:file://"), "{stdout}");
    assert!(stdout.contains("main.mjs"), "{stdout}");
    // Top-level await resolved.
    assert!(stdout.contains("AWAITED:42"), "{stdout}");
}

#[test]
fn resolves_parent_directory_imports_on_disk() {
    let out = run_file("sub/nested.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("nested:hello modules"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn streams_a_fetch_request_body_to_a_server() {
    // Real reqwest chunked upload → real runtime:http echo server, end to end.
    let out = run_file("fetch-stream-upload.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("UPLOAD_OK"), "{stdout}");
    assert!(stdout.contains("status:200"), "{stdout}");
}

#[test]
fn streams_an_http_server_response_to_a_client() {
    // Chunked download: a runtime:http handler returns a ReadableStream body
    // produced over time; the real reqwest client must see it incrementally
    // (several reads, no Content-Length) rather than as one buffered payload.
    let out = run_file("http-stream-response.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("STREAM_OK"), "{stdout}");
    assert!(stdout.contains("reads>1:true"), "{stdout}");
    assert!(stdout.contains("content-length:null"), "{stdout}");
    assert!(stdout.contains("x-mode:stream"), "{stdout}");
}

#[test]
fn streams_an_http_request_body_through_to_the_response() {
    // Proxy/echo: `new Response(request.body)` pipes the inbound stream back
    // out on the same request — concurrent pull + push, nothing buffered.
    let out = run_file("http-stream-echo.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("ECHO_OK"), "{stdout}");
    assert!(stdout.contains("status:200"), "{stdout}");
}

#[test]
fn a_request_body_kept_past_the_response_ends_rather_than_erroring() {
    // The host drops an undrained request body once the response is on its way,
    // so from the guest's side the body is simply over. Reading it afterwards
    // used to raise ERR_FOREIGN_HANDLE instead — the right answer to naming
    // another agent's request, the wrong one to naming your own after it was
    // answered.
    let out = run_file("http-body-after-respond.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("ENDED_CLEANLY"), "{stdout}");
    assert!(!stdout.contains("THREW"), "{stdout}");
}

#[test]
fn upgrades_a_websocket_on_the_http_server_that_is_serving_requests() {
    // D55: one port, one certificate, both protocols — how every peer runtime
    // does it, and what a deployment behind a single TLS endpoint needs. Also
    // pins that an upgraded socket is an ordinary connection: `broadcast()`
    // reaches it alongside the ones `runtime:websocket` `serve()` yields.
    let out = run_file("http-websocket-upgrade.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("http 200 api:/hello"), "{stdout}");
    assert!(stdout.contains("ws room:one"), "{stdout}");
    assert!(stdout.contains("broadcast room:two|room:two"), "{stdout}");
    // A Request the guest built has no connection behind it to take over.
    assert!(stdout.contains("refused TypeError"), "{stdout}");
    // Subprotocol negotiation: the client is told what the server picked, and a
    // protocol the client never offered is refused server-side rather than sent
    // for the client to reject.
    assert!(stdout.contains("refused-protocol"), "{stdout}");
    assert!(stdout.contains("protocol chat.v2"), "{stdout}");
}

#[test]
fn serves_http2_to_a_cleartext_client_that_opens_with_the_preface() {
    // A real `serve()` under the real binary, answering hand-written HTTP/2
    // frames over `runtime:net` — `fetch` would talk HTTP/1.1 to a cleartext
    // origin, so only raw frames can show the version detection actually fires
    // on the guest-facing path rather than only in the provider's own tests.
    let out = run_file("http2-h2c.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("H2C_HEADERS_FRAME"), "{stdout}");
    // The DATA payload is the handler's response, and the path proves the
    // request URL was rebuilt from `:authority` rather than a missing `Host`.
    assert!(stdout.contains("h2c-body:body-for:/h2c"), "{stdout}");
    // A second stream opened before the first was answered — multiplexing, on
    // the guest-facing path rather than only in the provider's tests.
    assert!(stdout.contains("h2c-second:body-for:/second"), "{stdout}");
    // A request body carried as DATA frames reached the handler's `text()`.
    assert!(stdout.contains("h2c-post:echo:uploaded-bytes"), "{stdout}");
    // The stream cap is advertised in the server's SETTINGS, so a client knows
    // it before opening anything. This is the documented number, read off the
    // wire rather than from the constant that set it.
    assert!(stdout.contains("h2c-max-streams:256"), "{stdout}");
    // …and the same port still answers an HTTP/1.1 `fetch`, which is the
    // mixed-version state every existing deployment is in.
    assert!(stdout.contains("h1-body:body-for:/http1"), "{stdout}");
}

#[test]
fn disconnects_stalled_clients_and_leaves_working_ones_alone() {
    // Real sockets under the real binary: a peer that goes quiet on a socket is
    // something `fetch` will never do, so this is the only place the timeouts
    // can be shown to act on the guest-facing path rather than in the
    // provider's own tests.
    let out = run_file("http-timeouts.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    // Connect and say nothing — the cheapest hold on a descriptor there is.
    assert!(stdout.contains("silent-closed:true"), "{stdout}");
    // A request head that starts and never finishes — slowloris.
    assert!(stdout.contains("dribble-closed:true"), "{stdout}");
    // …while the same server answers a real request normally.
    assert!(stdout.contains("request-ok:200:guarded"), "{stdout}");
    // Off means off: the identical stall survives when they are disabled.
    assert!(stdout.contains("disabled-closed:false"), "{stdout}");
    // And a bad value is a TypeError at the call, not a bound port.
    assert!(stdout.contains("bad-option:TypeError"), "{stdout}");
    assert!(stdout.contains("TIMEOUTS_OK"), "{stdout}");
}

/// `serve`'s documented failure contract, against the real server: a thrown
/// handler *and* a non-Response return are both a 500. The second used to be
/// coerced with `String(value)` and sent as a 200, so `return { ok: true }`
/// shipped "[object Object]" as a successful response.
#[test]
fn a_handler_that_throws_or_returns_a_non_response_is_a_500() {
    let out = run_file("http-handler-contract.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for path in [
        "/throw",
        "/reject",
        "/string",
        "/object",
        "/null",
        "/undefined",
    ] {
        assert!(
            s.contains(&format!("{path} status:500 body:\"Internal Server Error\"")),
            "{path} should be a 500 in:\n{s}",
        );
    }
    // A real Response is untouched.
    assert!(s.contains("/ok status:200 body:\"fine\""), "{s}");
    // The client learns nothing about the handler's mistake…
    assert!(s.contains("leak:false"), "{s}");
    // …but the developer does, on stderr.
    let err = stderr(&out);
    assert!(
        err.contains("handler blew up"),
        "the thrown error is reported: {err}"
    );
    assert!(
        err.contains("instead of a Response"),
        "the non-Response return is reported: {err}",
    );
    assert!(s.contains("HANDLER_CONTRACT_OK"), "{s}");
}

/// Fetch network errors reject with `TypeError`, which is how a caller tells a
/// transport failure from a programming one. Aborts and capability denials are
/// not network errors and keep their own types.
#[test]
fn fetch_reports_network_failures_as_type_errors() {
    let out = run_file("fetch-network-errors.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "refused:TypeError:TypeError:ERR_CONNECTION_REFUSED",
        "badscheme:TypeError:TypeError:",
        "dns:TypeError:TypeError:ERR_DNS",
        // The stable `code` survives the rewrap — guests branch on it.
        "loop:TypeError:TypeError:ERR_TOO_MANY_REDIRECTS",
        "redirect-error-mode:TypeError:TypeError:",
        "relative:TypeError:TypeError:",
        // Not network errors.
        "aborted:DOMException:AbortError:",
        "timeout:DOMException:TimeoutError:",
        "ok:200",
        "NETWORK_ERRORS_OK",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// The initiator of a WebSocket close must be told what it asked for. A client
/// calling `close(4001, "bye")` used to report 1006 / `wasClean: false` to its
/// own handler while the peer received 4001 — and 1006 means "dropped without a
/// close frame", so every clean shutdown read as a failure to reconnect logic.
#[test]
fn a_websocket_close_reports_the_same_code_at_both_ends() {
    let out = run_file("websocket-close-codes.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "custom client:4001/bye/true server:4001/bye/true",
        "normal client:1000//true server:1000//true",
        // No code supplied means no status was sent, which is what 1005 reports.
        "nocode client:1005//true server:1005//true",
        // …and a close the peer initiates is passed through unchanged.
        "server-initiated client:4002/server-said-so/true",
        "WS_CLOSE_OK",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// `runtime:net` port validation and failure shape, and `runtime:process`
/// `args` reporting as the frozen value the docs describe.
#[test]
fn net_validates_ports_and_reports_binds_as_socket_errors() {
    let out = run_file("net-validation.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        // A port that is not a port used to silently become 0.
        "negative:TypeError:SocketError: invalid port",
        "missing:TypeError:SocketError: invalid port",
        "nan:TypeError:SocketError: invalid port",
        "toobig:TypeError:SocketError: invalid port",
        "zero-connect:TypeError:SocketError: invalid port",
        // …but `listen(0)` is the documented ephemeral-port request.
        "listen-zero:true",
        // A bind failure carries the same SocketError shape a connect does.
        "privileged:TypeError:true",
        // `Object.isFrozen` asks [[IsExtensible]] first, which used to bypass
        // the lazy seeding and report the empty, still-extensible array.
        "args-frozen:true",
        "args-array:true",
        "args-push:threw",
        "NET_VALIDATION_OK",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// `SO_REUSEPORT` through both doors: several listeners share one port, which
/// is how a server runs across cores without a front proxy and how one is
/// replaced without dropping connections.
#[cfg(unix)]
#[test]
fn several_listeners_can_share_a_port_with_reuse_port() {
    let out = run_file("reuse-port.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "http-shared:true",
        "http-answered:true",
        // Without the option the same bind is still refused — so the option is
        // doing the work rather than the port having been free.
        "http-exclusive:ERR_ADDRESS_IN_USE",
        "net-shared:true",
        "net-exclusive:ERR_ADDRESS_IN_USE",
        // A non-boolean is a mistake at the call, not a silently ignored option.
        "http-bad-option:TypeError",
        "net-bad-option:TypeError",
        "REUSE_PORT_OK",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// What a failed bind tells the program. `finished` used to *resolve*, so a
/// server that never bound was indistinguishable from one that ran and shut
/// down cleanly, and the error was an uncoded string with nothing to branch on.
#[test]
fn a_failed_bind_rejects_both_addr_and_finished_with_a_coded_error() {
    let out = run_file("serve-bind-failure.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "addr:ERR_ADDRESS_IN_USE:true",
        "finished:ERR_ADDRESS_IN_USE",
        // The port's real owner is unaffected…
        "held-still-serving:held",
        // …and a clean shutdown still resolves `finished`, so rejecting is not
        // the new default for every server.
        "clean-finished:resolved",
        "BIND_FAILURE_OK",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// A top-level throw is fatal even when the program has already started a
/// server. The failure was only checked after the drive loop returned, and a
/// listener keeps that loop alive forever — so the exception was discarded and
/// the process ran on, serving, with nothing reported.
#[test]
fn a_top_level_throw_is_reported_even_with_a_server_running() {
    let out = run_file("throw-with-server.mjs");
    assert!(!out.status.success(), "the process must fail");
    let err = stderr(&out);
    assert!(err.contains("uncaught exception"), "{err}");
    assert!(err.contains("top-level failure while serving"), "{err}");
}

/// A failure in a program that never quiesces is reported when it happens.
///
/// Failures were collected and printed only when the drive returned, and a
/// listening server keeps that loop alive — so a long-running program's
/// failures were invisible for its whole life and surfaced at exit, if ever.
#[test]
fn an_unhandled_rejection_is_reported_while_the_server_is_still_running() {
    let out = run_file("rejection-while-serving.mjs");
    assert!(!out.status.success(), "the run must fail");
    let s = stdout(&out);
    let err = stderr(&out);
    assert!(err.contains("error: unhandled promise rejection"), "{err}");
    assert!(err.contains("TypeError: failed while serving"), "{err}");
    // The server kept running afterwards — the report is not the end of the
    // program, it is news delivered during it.
    assert!(s.contains("MARK_BEFORE") && s.contains("MARK_AFTER"), "{s}");
    // …and the exit status still reflects it, without repeating the message.
    assert!(
        err.contains("1 unhandled failure — reported above"),
        "{err}"
    );
}

#[test]
fn tells_a_handler_which_peer_a_request_came_from() {
    // A real accept() under the real binary: the address a handler is given has
    // to come from the socket, which no in-process test of the prelude can
    // show on its own.
    let out = run_file("http-peer-address.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(
        stdout.contains("peer:tcp/127.0.0.1 hasPort:true"),
        "{stdout}"
    );
    // A forged forwarding header changes nothing about the reported peer.
    assert!(stdout.contains("forged-ignored:true"), "{stdout}");
    // …but it still reaches the handler, so a deployment can trust it itself.
    assert!(stdout.contains("header-delivered:198.51.100.9"), "{stdout}");
    assert!(stdout.contains("one-arg:ignored"), "{stdout}");
    assert!(stdout.contains("PEER_OK"), "{stdout}");
}

#[test]
fn holds_connections_over_the_cap_back_rather_than_refusing_them() {
    // A real accept loop with a real kernel backlog behind it: the cap works by
    // *not accepting*, which is only observable where there is something for
    // the connection to wait in.
    let out = run_file("http-max-connections.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("first:HTTP/1.1 200 OK"), "{stdout}");
    // Connected, but not served while the only slot is taken.
    assert!(stdout.contains("second-while-full:silent"), "{stdout}");
    // …and served once it frees, which is what makes this a queue and not a
    // refusal.
    assert!(
        stdout.contains("second-after-free:HTTP/1.1 200 OK"),
        "{stdout}"
    );
    assert!(stdout.contains("CAP_OK"), "{stdout}");
}

#[test]
fn sends_response_trailers_from_a_handler() {
    // Read off a raw socket under the real binary: `fetch` drops trailers — in
    // every runtime — so nothing short of the wire can show these arrived.
    let out = run_file("http-trailers.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("buffered-trailer:true"), "{stdout}");
    // The `Trailer` header is added for HTTP/1.1 when the names are known in
    // time, so a handler does not have to know that rule.
    assert!(stdout.contains("buffered-declared:true"), "{stdout}");
    // A trailered response cannot use Content-Length; there would be nowhere to
    // put the trailer section.
    assert!(stdout.contains("buffered-chunked:true"), "{stdout}");
    // The gRPC shape: trailers promised at respond time, sent after the body.
    assert!(stdout.contains("streamed-body:true"), "{stdout}");
    assert!(stdout.contains("streamed-trailer:true"), "{stdout}");
    assert!(stdout.contains("plain-ok:true"), "{stdout}");
    assert!(stdout.contains("bad-arg:TypeError"), "{stdout}");
    assert!(stdout.contains("TRAILERS_OK"), "{stdout}");
}

#[test]
fn reads_response_trailers_with_fetch() {
    // The round trip on one runtime: a handler attaches trailers and `fetch`
    // reads them back — the gRPC shape, status after the body.
    let out = run_file("http-trailers-client.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("body:payload"), "{stdout}");
    assert!(stdout.contains("status:0 message:fine"), "{stdout}");
    // Promised trailers arrive the same way from the client's side.
    assert!(stdout.contains("failed-status:13"), "{stdout}");
    // No trailers is empty Headers, not a rejection — on a response that had
    // none, on one the guest built itself, and on a body that was cancelled
    // rather than read (which must settle rather than hang).
    assert!(stdout.contains("none:0"), "{stdout}");
    assert!(stdout.contains("local:0"), "{stdout}");
    assert!(stdout.contains("cancelled:0"), "{stdout}");
    assert!(stdout.contains("bad-arg:TypeError"), "{stdout}");
    assert!(stdout.contains("CLIENT_TRAILERS_OK"), "{stdout}");
}

#[test]
fn honours_every_fetch_redirect_mode_over_the_wire() {
    // Real 3xx responses from a real runtime:http server through the real
    // reqwest transport — the stub transport in the runtime's own tests cannot
    // prove that the redirect policy actually reaches the client.
    let out = run_file("fetch-redirect.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(
        stdout.contains("FOLLOW status:200 redirected:true landed:true body:landed"),
        "{stdout}"
    );
    assert!(
        stdout.contains("MANUAL status:302 redirected:false location:true"),
        "{stdout}"
    );
    assert!(stdout.contains("ERROR threw:TypeError"), "{stdout}");
    assert!(stdout.contains("DIRECT redirected:false"), "{stdout}");
    assert!(
        stdout.contains("LOOP code:ERR_TOO_MANY_REDIRECTS"),
        "{stdout}"
    );
    assert!(stdout.contains("REDIRECT_OK"), "{stdout}");
}

#[test]
fn decodes_compressed_response_bodies_and_identifies_itself() {
    // Real compressed bytes from a real runtime:http server: the guest must see
    // the decoded body, and the headers describing the compressed form must not
    // survive to describe bytes it never sees.
    let out = run_file("fetch-content-encoding.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    for coding in ["gzip", "deflate", "br"] {
        assert!(
            stdout.contains(&format!(
                "DECODE {coding} ok:true content-encoding:null content-length:null"
            )),
            "{stdout}"
        );
    }
    assert!(
        stdout.contains("ACCEPT gzip:true br:true deflate:true"),
        "{stdout}"
    );
    assert!(stdout.contains("EXPLICIT ok:true"), "{stdout}");
    assert!(
        stdout.contains("UNKNOWN body:true content-encoding:zstd"),
        "{stdout}"
    );
    assert!(
        stdout.contains("UA default:true matches-navigator:true"),
        "{stdout}"
    );
    assert!(stdout.contains("UA override:true"), "{stdout}");
    assert!(stdout.contains("ENCODING_OK"), "{stdout}");
}

#[test]
fn a_server_handler_learns_the_client_hung_up() {
    // Needs a real socket to close, so it cannot be an in-process test. Also
    // asserts the opposite: a request that completed leaves its signal
    // unaborted, and the process exits — a disconnect watch that never settled
    // would hold the driven loop open forever.
    let out = run_file("http-request-signal.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("PLAIN body:plain done"), "{stdout}");
    assert!(
        stdout.contains("ABORT reason:AbortError promptly:true"),
        "{stdout}"
    );
    assert!(stdout.contains("QUICK body:quick done"), "{stdout}");
    assert!(stdout.contains("QUICK aborted:false"), "{stdout}");
    assert!(stdout.contains("SIGNAL_OK"), "{stdout}");
}

#[test]
fn a_deadline_ends_a_request_the_server_never_answers() {
    // The connect timeout cannot help here — the peer accepts and then goes
    // silent — so this pins the documented answer for bounding a whole request.
    let out = run_file("fetch-timeout.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(
        stdout.contains("TIMEOUT name:TimeoutError promptly:true"),
        "{stdout}"
    );
    assert!(stdout.contains("NORMAL body:prompt"), "{stdout}");
    assert!(stdout.contains("TIMEOUT_OK"), "{stdout}");
}

#[test]
fn runs_an_inline_module_snippet() {
    let out = esrun()
        .arg("-e=console.log('inline', 6 * 7)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("inline 42"), "{}", stdout(&out));
}

#[test]
fn inline_snippet_supports_top_level_await() {
    let out = esrun()
        .arg("-e=const x = await Promise.resolve(5); console.log('awaited', x)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("awaited 5"), "{}", stdout(&out));
}

#[test]
fn top_level_throw_fails_with_uncaught_report() {
    let out = run_file("throws.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    // Side effects before the throw still ran...
    assert!(stdout(&out).contains("before throw"), "{}", stdout(&out));
    // ...and the throw is reported once as Uncaught.
    let stderr = stderr(&out);
    assert!(stderr.contains("error: uncaught exception"), "{stderr}");
    assert!(stderr.contains("fixture boom"), "{stderr}");
    assert!(stderr.contains("at file://"), "{stderr}");
}

#[test]
fn missing_import_is_a_load_error() {
    let out = run_file("missing.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("module loading failed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn uninstalled_bare_package_is_not_found() {
    // bare.mjs imports "lodash", which is not in any node_modules here.
    let out = run_file("bare.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("cannot find package"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn nonexistent_entry_file_errors_cleanly() {
    let out = run_file("no-such-file.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn resolves_a_bare_esm_package_from_node_modules() {
    let out = run_file("uses-package.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("hi world from greeter"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn rejects_a_commonjs_package() {
    let out = run_file("uses-cjs-package.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("CommonJS"), "{}", stderr(&out));
}

#[test]
fn dynamic_import_resolves_relative_and_node_modules() {
    let out = run_file("dynamic.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    assert!(stdout.contains("hi dynamic from greeter"), "{stdout}");
}

#[test]
fn all_esm_export_import_patterns_work() {
    // esm/consumer.mjs exercises every standardized export/import form against
    // the export fixtures and throws on any mismatch.
    let out = run_file("esm/consumer.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ESM-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn node_modules_export_patterns_work() {
    // A node_modules package with an exports map: ".", a subpath, and a wildcard
    // subpath, with named + default exports and an internal re-export.
    let out = run_file("esm/consumer-pkg.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("PKG-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn node_modules_export_conditions_work() {
    // Condition matching in author order, unasserted conditions skipped, array
    // fallbacks, nested conditions, and a `null`-withdrawn subpath.
    let out = run_file("esm/consumer-conditions.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("COND-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn private_imports_and_self_reference_work() {
    // `#specifier` through the package's own "imports" map (path, pattern, and
    // another package), plus importing this package by its own name.
    let out = run_file("esm/consumer-private.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("PRIVATE-SUITE-OK"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_process_exposes_env_args_platform_cwd() {
    let out = esrun()
        .env("ESRUN_TEST_VAR", "hello")
        .arg(format!(
            "-e={}",
            "import { env, args, platform, arch, cwd } from 'runtime:process'; \
             console.log(env.ESRUN_TEST_VAR, platform, arch, args.join(','), typeof cwd());",
        ))
        .arg("alpha")
        .arg("beta")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("hello"), "env: {s}");
    // platform is the OS-native std value (linux/macos/windows).
    assert!(
        s.contains("linux") || s.contains("macos") || s.contains("windows"),
        "platform: {s}"
    );
    // arch is the OS-native std value (x86_64/aarch64/arm/...).
    assert!(
        s.contains("x86_64") || s.contains("aarch64") || s.contains("arm"),
        "arch: {s}"
    );
    assert!(s.contains("alpha,beta"), "args: {s}"); // user args only, in order
    assert!(s.contains("string"), "cwd: {s}");
}

#[test]
fn runtime_process_exit_sets_exit_code() {
    let out = esrun()
        .arg("-e=import { exit } from 'runtime:process'; console.log('before'); exit(5); console.log('after');")
        .output()
        .expect("spawn esrun");
    assert_eq!(out.status.code(), Some(5), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("before"), "{}", stdout(&out));
    assert!(
        !stdout(&out).contains("after"),
        "exit did not halt: {}",
        stdout(&out)
    );
}

#[test]
fn exit_after_a_top_level_await_still_exits() {
    // `exit()` terminates the isolate, which does not settle the module's
    // evaluation promise — so a program that had already awaited stayed
    // "pending" forever and the process hung, unless `exit()` happened to be
    // the very last statement. The shapes below are the ordinary ones: an early
    // return from a loop, and a guard clause.
    for (label, snippet) in [
        (
            "loop",
            "for (const x of [1]) { await null; console.log('bye'); exit(9); }",
        ),
        (
            "guard",
            "await null; if (true) { console.log('bye'); exit(9); }\nconsole.log('after');",
        ),
        (
            "timer",
            "setTimeout(() => { console.log('bye'); exit(9); }, 5);\nsetTimeout(() => {}, 60_000);",
        ),
    ] {
        let out = esrun()
            .arg(format!(
                "-e=import {{ exit }} from 'runtime:process';\n{snippet}"
            ))
            .output()
            .expect("spawn esrun");
        assert_eq!(out.status.code(), Some(9), "{label}: {}", stderr(&out));
        assert!(stdout(&out).contains("bye"), "{label}: {}", stdout(&out));
        assert!(
            !stdout(&out).contains("after"),
            "{label}: exit did not halt: {}",
            stdout(&out)
        );
    }
}

#[test]
fn a_dynamic_import_does_not_wait_for_an_unrelated_timer() {
    // A linked import()'s promise is settled by the *next* tick, so a driver
    // that parked first charged the whole park to the import: with a 3s timer
    // pending, `await import(…)` took 3s. The margin is wide on purpose — the
    // assertion is "did not wait for the timer", not a latency budget.
    let out = esrun()
        .arg(
            "-e=const t0 = Date.now();\
              const timer = setTimeout(() => {}, 3000);\
              await import('runtime:path');\
              clearTimeout(timer);\
              console.log('ELAPSED', Date.now() - t0);",
        )
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    let elapsed: u64 = stdout
        .split_whitespace()
        .last()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no elapsed in {stdout}"));
    assert!(elapsed < 1_000, "import waited for the timer: {elapsed}ms");
}

// POSIX-only: separators/roots are platform-specific and the CI test job runs
// on Linux (macOS is also POSIX). Windows path semantics are exercised by hand.
#[cfg(unix)]
#[test]
fn runtime_path_exposes_modern_surface() {
    let out = esrun()
        .arg(format!(
            "-e={}",
            "import * as p from 'runtime:path'; const o=(k,v)=>console.log(k+'='+v);\
             o('sep',p.sep); o('delimiter',p.delimiter);\
             o('join',p.join('a','b','..','c/d/'));\
             o('normalize',p.normalize('/a/./b/../c'));\
             o('isAbs',p.isAbsolute('/a')+','+p.isAbsolute('a'));\
             o('dirname',p.dirname('/a/b/c.txt'));\
             o('basename',p.basename('/a/b/c.txt'));\
             o('extname',p.extname('archive.tar.gz'));\
             o('relative',p.relative('/a/b/c','/a/x/y'));\
             o('parse',JSON.stringify(p.parse('/a/b/c.txt')));\
             o('resolveAbs',p.resolve('/x','y','z'));\
             o('fromFileURL',p.fromFileURL('file:///a/b%20c.txt'));\
             o('toFileURL',p.toFileURL('/a/b c.txt').href);\
             o('keepSlash',p.normalize('a/b/'));\
             o('noSlash',p.normalize('a/b'));\
             o('dotSlash',p.normalize('./'));\
             o('dot',p.normalize('.'));\
             o('upSlash',p.normalize('a/b/../'));\
             o('joinSlash',p.join('a','b/'));\
             o('resolveSlash',p.resolve('/a','b/'));\
             o('resolveRoot',p.resolve('/'));",
        ))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "sep=/",
        "delimiter=:",
        // A trailing separator is kept: it says "this names a directory".
        "join=a/c/d/\n",
        "normalize=/a/c",
        "isAbs=true,false",
        "dirname=/a/b",
        "basename=c.txt",
        "extname=.gz",
        "relative=../../x/y",
        "parse={\"root\":\"/\",\"dir\":\"/a/b\",\"base\":\"c.txt\",\"name\":\"c\",\"ext\":\".txt\"}",
        "resolveAbs=/x/y/z",
        "fromFileURL=/a/b c.txt",
        "toFileURL=file:///a/b%20c.txt",
        // normalize and join keep a trailing separator; resolve drops it,
        // because it answers "which location is this" and a location is the
        // same one however it was spelled. The root is itself a separator.
        "keepSlash=a/b/\n",
        "noSlash=a/b\n",
        "dotSlash=./\n",
        "dot=.\n",
        "upSlash=a/\n",
        "joinSlash=a/b/\n",
        "resolveSlash=/a/b\n",
        "resolveRoot=/\n",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

/// The jail root is not a target. `base.join("")` is `base`, so an empty path
/// used to resolve to the root and `remove('', { recursive: true })` deleted the
/// whole project — this runs the real binary against a real directory to prove
/// the guard holds end to end, and that reads of `.` still work.
#[test]
fn the_jail_root_cannot_be_removed_renamed_or_chmodded() {
    let tmp = std::env::temp_dir().join(format!("esrun-rootguard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("data")).expect("mktemp");
    std::fs::write(tmp.join("data/db.txt"), b"important").expect("seed");
    let script = "import { remove, chmod, rename, truncate, write, mkdir, stat, readDir } from 'runtime:fs';\
        const chk = async (label, fn) => { try { await fn(); console.log(label + '=SUCCEEDED'); }\
          catch (e) { console.log(label + '=' + e.code); } };\
        await chk('EMPTY_REMOVE', () => remove('', { recursive: true }));\
        await chk('EMPTY_WRITE', () => write('', 'x'));\
        await chk('EMPTY_MKDIR', () => mkdir(''));\
        await chk('EMPTY_TRUNCATE', () => truncate(''));\
        await chk('DOT_REMOVE', () => remove('.', { recursive: true }));\
        await chk('DOTSLASH_REMOVE', () => remove('./', { recursive: true }));\
        await chk('UPWARD_REMOVE', () => remove('data/..', { recursive: true }));\
        await chk('DOT_CHMOD', () => chmod('.', 0));\
        await chk('DOT_RENAME', () => rename('.', 'moved'));\
        console.log('READ_DOT=' + (await stat('.')).isDir);\
        console.log('LIST=' + (await readDir('.')).map(e => e.name).sort().join(','));\
        await write('inside.txt', 'ok');\
        console.log('WROTE_INSIDE=' + (await stat('inside.txt')).size);";
    let out = esrun()
        .current_dir(&tmp)
        .arg(format!("-e={script}"))
        .output()
        .expect("spawn esrun");
    let s = stdout(&out);
    let survived = tmp.join("data/db.txt").exists();
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    for expected in [
        "EMPTY_REMOVE=ERR_INVALID_PATH",
        "EMPTY_WRITE=ERR_INVALID_PATH",
        "EMPTY_MKDIR=ERR_INVALID_PATH",
        "EMPTY_TRUNCATE=ERR_INVALID_PATH",
        "DOT_REMOVE=ERR_INVALID_PATH",
        "DOTSLASH_REMOVE=ERR_INVALID_PATH",
        "UPWARD_REMOVE=ERR_INVALID_PATH",
        "DOT_CHMOD=ERR_INVALID_PATH",
        "DOT_RENAME=ERR_INVALID_PATH",
        // Reads of the root, and writes to entries inside it, are untouched.
        "READ_DOT=true",
        "LIST=data",
        "WROTE_INSIDE=2",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
    assert!(
        survived,
        "the seeded file under the jail root was destroyed"
    );
}

#[test]
fn runtime_fs_read_write_stat_and_jail() {
    // A scratch dir that becomes the jail root (no package.json there, so the
    // detected root is the dir itself); run with cwd set to it.
    let tmp = std::env::temp_dir().join(format!("esrun-fs-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mktemp");
    let script = "import { file, write, readDir, stat, mkdir, remove, exists } from 'runtime:fs';\
        await mkdir('sub', { recursive: true });\
        console.log('WROTE=' + await write('sub/a.txt', 'hi'));\
        console.log('TEXT=' + await file('sub/a.txt').text());\
        const s = await stat('sub/a.txt');\
        console.log('SIZE=' + s.size + ' ISFILE=' + s.isFile);\
        console.log('DIR=' + (await readDir('sub')).map(e => e.name).join(','));\
        console.log('EXISTS=' + await exists('sub/a.txt'));\
        await remove('sub', { recursive: true });\
        console.log('GONE=' + !(await exists('sub')));\
        try { await file('../escape.txt').text(); console.log('JAIL=open'); }\
        catch { console.log('JAIL=blocked'); }";
    let out = esrun()
        .current_dir(&tmp)
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "WROTE=2",
        "TEXT=hi",
        "SIZE=2 ISFILE=true",
        "DIR=a.txt",
        "EXISTS=true",
        "GONE=true",
        "JAIL=blocked",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

#[test]
fn runtime_fs_glob_covers_all_patterns() {
    // match() is pure (no FS), so the full pattern set runs without fixtures.
    let script = "import { Glob } from 'runtime:fs'; const m = (p, s) => new Glob(p).match(s);\
        const out = [];\
        out.push('q=' + m('???.ts','foo.ts') + ',' + m('???.ts','foobar.ts'));\
        out.push('star=' + m('*.ts','index.ts') + ',' + m('*.ts','src/index.ts'));\
        out.push('globstar=' + m('**/*.ts','src/index.ts'));\
        out.push('class=' + m('ba[rz].ts','bar.ts') + ',' + m('ba[rz].ts','bat.ts'));\
        out.push('range=' + m('f[a-c].ts','fb.ts') + ',' + m('f[a-c].ts','fz.ts'));\
        out.push('negbang=' + m('f[!o]o.ts','fao.ts') + ',' + m('f[!o]o.ts','foo.ts'));\
        out.push('negcaret=' + m('f[^o]o.ts','fao.ts') + ',' + m('f[^o]o.ts','foo.ts'));\
        out.push('brace=' + m('{a,b}.ts','a.ts') + ',' + m('{a,b}.ts','c.ts'));\
        out.push('not=' + m('!index.ts','a.ts') + ',' + m('!index.ts','index.ts'));\
        out.push('escape=' + m('\\\\!x.ts','!x.ts') + ',' + m('\\\\!x.ts','x.ts'));\
        console.log(out.join('\\n'));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "q=true,false",
        "star=true,false",
        "globstar=true",
        "class=true,false",
        "range=true,false",
        "negbang=true,false",
        "negcaret=true,false",
        "brace=true,false",
        "not=true,false",
        // `\` escapes a special character everywhere except Windows, where it is
        // the path separator and globset disables escaping for that reason (the
        // same call Node's minimatch makes with `windowsPathsNoEscape`). The
        // pattern is then a path, and matches nothing here.
        if cfg!(windows) {
            "escape=false,false"
        } else {
            "escape=true,false"
        },
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

#[test]
fn runtime_net_tcp_echo_roundtrip() {
    // Loopback: a one-shot echo server + a client, exercising connect/listen/
    // accept, the Socket read/write streams, half-close, and clean shutdown
    // (the process must exit, not hang).
    let script = "import { connect, listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        (async () => {\
          for await (const conn of server) {\
            const w = conn.writable.getWriter();\
            for await (const chunk of conn.readable) await w.write(chunk);\
            await w.close();\
            await server.close();\
          }\
        })();\
        const sock = connect({ hostname: '127.0.0.1', port });\
        const w = sock.writable.getWriter();\
        await w.write(new TextEncoder().encode('ping'));\
        await w.close();\
        let out = ''; const dec = new TextDecoder();\
        for await (const chunk of sock.readable) out += dec.decode(chunk);\
        console.log('NET:' + out);";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("NET:ping"), "{}", stdout(&out));
}

#[test]
fn runtime_net_starttls_surface_and_guards() {
    // The startTls() JS surface: a plain socket can't be upgraded, an unknown
    // secureTransport is rejected, and a "starttls" socket opens (upgradable).
    // Also asserts the WinterTC SocketError shape (a TypeError whose message is
    // prefixed "SocketError: ", marked "+SE" below) across both a synchronous
    // option-validation throw and a runtime failure (a refused connect rejecting
    // .opened — exercising the socketOp() op-rejection wrapper). The TLS
    // handshake itself is covered by hermetic provider tests (the CLI trusts the
    // public webpki roots, so a loopback self-signed cert can't be exercised here).
    let script = "import { connect, listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        const a = connect({ hostname: '127.0.0.1', port });\
        const tag = (e) => e.constructor.name + (e.message.startsWith('SocketError: ') ? '+SE' : '');\
        let g1 = 'none';\
        try { a.startTls(); } catch (e) { g1 = tag(e); }\
        let g2 = 'none';\
        try { connect({ hostname: '127.0.0.1', port }, { secureTransport: 'x' }); }\
        catch (e) { g2 = tag(e); }\
        const b = connect({ hostname: '127.0.0.1', port }, { secureTransport: 'starttls' });\
        let g3 = 'none';\
        try { const c = connect('127.0.0.1:1'); await c.opened; } catch (e) { g3 = tag(e); }\
        console.log('STARTTLS:' + g1 + ':' + g2 + ':' + g3 + ':' + (b.upgraded === false));\
        await a.close('done'); await b.close(); await server.close();";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("STARTTLS:TypeError+SE:TypeError+SE:TypeError+SE:true"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_net_listener_close_ends_accept_loop() {
    // A detached `for await (conn of server)` loop, closed from the main flow,
    // must terminate (and let the process exit) — the parked accept resolves to
    // null. Regression for the listener-close cancellation.
    let script = "import { listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        await server.addr;\
        let ended = false;\
        const loop = (async () => { for await (const _ of server) {} ended = true; })();\
        await server.close();\
        await loop;\
        console.log('CLOSED:' + ended);";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("CLOSED:true"), "{}", stdout(&out));
}

#[test]
fn runtime_net_half_open_and_combined_address() {
    // allowHalfOpen: the server FINs its write; the client (allowHalfOpen: true)
    // sees read EOF yet can still write — a default socket would be torn down.
    // Also checks SocketInfo.remoteAddress is the WinterTC "host:port" form.
    let script = "import { connect, listen } from 'runtime:net';\
        const enc = new TextEncoder(); const dec = new TextDecoder();\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        (async () => {\
          for await (const conn of server) {\
            const w = conn.writable.getWriter();\
            await w.write(enc.encode('hi'));\
            await w.close();\
            let got = '';\
            for await (const chunk of conn.readable) got += dec.decode(chunk);\
            console.log('GOT:' + got);\
            await server.close();\
          }\
        })();\
        const sock = connect({ hostname: '127.0.0.1', port }, { allowHalfOpen: true });\
        const info = await sock.opened;\
        let out = '';\
        for await (const chunk of sock.readable) out += dec.decode(chunk);\
        const w = sock.writable.getWriter();\
        await w.write(enc.encode('after'));\
        await w.close();\
        console.log('HALF:' + out + ':' + info.remoteAddress.includes(':'));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("HALF:hi:true"), "{s}");
    assert!(
        s.contains("GOT:after"),
        "half-open write did not reach peer:\n{s}"
    );
}

#[test]
fn runtime_net_listen_tls_surface_and_bind() {
    // Server-side TLS termination on listen(): secureTransport: "on" needs a
    // cert+key (TypeError without), an unknown mode is rejected, and a real PEM
    // cert/key binds a TLS listener (proving cert/key bytes thread through the op
    // and the ServerConfig builds). The handshake itself is covered by hermetic
    // provider tests — the CLI trusts the public webpki roots, so a loopback
    // self-signed cert can't be verified end to end here.
    let script = r#"import { listen } from 'runtime:net';
        const cert = `-----BEGIN CERTIFICATE-----
MIIBfzCCASWgAwIBAgIUW7VE71ojFZyS30mgfs6/aXgiVxkwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MDYxODIwMzUyNFoYDzIxMjYwNTI1
MjAzNTI0WjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAARMmfJSPruUifoGAbRY3gh/Sss+GDYDVXKwlHaaiSsPtueuWJ1GwC4P
m9kbriVs1/9YTXpKdjsPga00am7iwK7co1MwUTAdBgNVHQ4EFgQU+R9EaUJyGVun
alMb5fKe5Hlx53QwHwYDVR0jBBgwFoAU+R9EaUJyGVunalMb5fKe5Hlx53QwDwYD
VR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiEA/jRmQTJnYabU4zgNrGeI
bO2qBiYf5YwjN+WfeyP3ecUCIHFmUGu2HNVscjPnIlBRJpeBIw29Xm8r+ddP95M+
hMU9
-----END CERTIFICATE-----`;
        const key = `-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgoI+sRkHefoxwbeyv
0GUmYblUBM3eh+YRg6PRzrJEB5yhRANCAARMmfJSPruUifoGAbRY3gh/Sss+GDYD
VXKwlHaaiSsPtueuWJ1GwC4Pm9kbriVs1/9YTXpKdjsPga00am7iwK7c
-----END PRIVATE KEY-----`;
        let r = '';
        const tag = (e) => e.constructor.name + (e.message.startsWith('SocketError: ') ? '+SE' : '');
        try { listen({ port: 0, secureTransport: 'on' }); r += 'NOCERT:no-throw'; }
        catch (e) { r += 'NOCERT:' + tag(e); }
        try { listen({ port: 0, secureTransport: 'bogus' }); r += ':MODE:no-throw'; }
        catch (e) { r += ':MODE:' + tag(e); }
        const server = listen({ hostname: '127.0.0.1', port: 0, secureTransport: 'on', cert, key, alpn: ['h2'] });
        const { port } = await server.addr;
        r += ':BOUND:' + (port > 0);
        await server.close();
        console.log(r);"#;
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("NOCERT:TypeError+SE:MODE:TypeError+SE:BOUND:true"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_net_udp_datagram_roundtrip() {
    // Loopback UDP: bind/send/receive, the sender's address arriving with the
    // datagram (nothing told the server that port in advance), a reply to it,
    // message boundaries kept across three sends — including a zero-length one,
    // which is a message and not an EOF — and a `for await` loop that ends at
    // close() so the process exits rather than hanging.
    let script = "import { bind } from 'runtime:net';\
        const enc = new TextEncoder(); const dec = new TextDecoder();\
        const server = bind({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        const client = bind({ hostname: '127.0.0.1', port: 0 });\
        const sent = await client.send(enc.encode('ping'), { hostname: '127.0.0.1', port });\
        const first = await server.receive();\
        await server.send(enc.encode('pong'), `${first.address}:${first.port}`);\
        const back = await client.receive();\
        for (const body of ['one', '', 'three'])\
          await client.send(enc.encode(body), `127.0.0.1:${port}`);\
        let parts = [];\
        for (let i = 0; i < 3; i++) parts.push(dec.decode((await server.receive()).data));\
        const loop = (async () => { for await (const _ of server) {} return 'ended'; })();\
        await server.close(); await client.close();\
        console.log('UDP:' + sent + ':' + dec.decode(first.data) + ':' + first.address\
          + ':' + dec.decode(back.data) + ':' + parts.join('|') + ':' + (await loop)\
          + ':' + (await server.receive()));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("UDP:4:ping:127.0.0.1:pong:one||three:ended:null"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_net_udp_connect_and_validation() {
    // A connected socket sends with no address and hears only its peer; an
    // unconnected one refuses a send with no destination. Plus the option and
    // port validation, all in the WinterTC SocketError shape ("+SE").
    let script = "import { bind } from 'runtime:net';\
        const enc = new TextEncoder(); const dec = new TextDecoder();\
        const tag = (e) => e.constructor.name + (e.message.startsWith('SocketError: ') ? '+SE' : '');\
        const server = bind({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        const client = bind({ hostname: '127.0.0.1', port: 0 });\
        let r = '';\
        try { await client.send(enc.encode('nowhere')); r += 'NOPEER:no-throw'; }\
        catch (e) { r += 'NOPEER:' + tag(e); }\
        const info = await client.connect({ hostname: '127.0.0.1', port });\
        await client.send(enc.encode('connected'));\
        r += ':GOT:' + dec.decode((await server.receive()).data);\
        r += ':PEER:' + (info.remoteAddress === `127.0.0.1:${port}`);\
        try { bind({ port: 70000 }); r += ':PORT:no-throw'; } catch (e) { r += ':PORT:' + tag(e); }\
        try { bind({ port: 0, ttl: 999 }); r += ':TTL:no-throw'; } catch (e) { r += ':TTL:' + tag(e); }\
        try { bind({ port: 0, broadcast: 'yes' }); r += ':OPT:no-throw'; } catch (e) { r += ':OPT:' + tag(e); }\
        try { await client.joinMulticast('127.0.0.1'); r += ':GROUP:no-throw'; }\
        catch (e) { r += ':GROUP:' + tag(e); }\
        await server.close(); await client.close();\
        try { await client.send(enc.encode('after')); r += ':CLOSED:no-throw'; }\
        catch (e) { r += ':CLOSED:' + tag(e); }\
        console.log(r);";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "NOPEER:TypeError+SE:GOT:connected:PEER:true:PORT:TypeError+SE:TTL:TypeError+SE\
             :OPT:TypeError+SE:GROUP:TypeError+SE:CLOSED:TypeError+SE"
        ),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_net_udp_multicast_and_socket_options() {
    // The options that only exist on a datagram socket: reuseAddress (two
    // sockets on one port), broadcast, the two TTLs, and a multicast group
    // joined and left on the loopback interface. An administratively scoped
    // group (RFC 2365), so nothing else on the machine is a member.
    let script = "import { bind } from 'runtime:net';\
        const enc = new TextEncoder(); const dec = new TextDecoder();\
        const opts = { hostname: '0.0.0.0', reuseAddress: true, broadcast: true,\
          ttl: 4, multicastTtl: 1, multicastLoopback: true };\
        const first = bind({ ...opts, port: 0 });\
        const { port } = await first.addr;\
        const second = bind({ ...opts, port });\
        await second.addr;\
        await first.joinMulticast('239.255.42.98', { interface: '127.0.0.1' });\
        await second.joinMulticast('239.255.42.98', { interface: '127.0.0.1' });\
        const sender = bind({ hostname: '127.0.0.1', port: 0 });\
        await sender.send(enc.encode('announce'), `239.255.42.98:${port}`);\
        const heard = [dec.decode((await first.receive()).data), dec.decode((await second.receive()).data)];\
        await first.leaveMulticast('239.255.42.98', { interface: '127.0.0.1' });\
        await second.leaveMulticast('239.255.42.98', { interface: '127.0.0.1' });\
        await first.close(); await second.close(); await sender.close();\
        console.log('MCAST:' + (port > 0) + ':' + heard.join('|'));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("MCAST:true:announce|announce"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_http_serve_and_fetch_roundtrip() {
    // Loopback: serve() an echo-ish handler, fetch() it through the real HTTP
    // client, read body + a custom header, then stop the server so the process
    // exits cleanly (must not hang).
    let script = "import { serve } from 'runtime:http';\
        const server = serve({ hostname: '127.0.0.1', port: 0 }, async (req) => {\
          const who = await req.text();\
          return new Response('hello ' + (who || 'world'), {\
            status: 201, headers: { 'x-greeting': 'hi' },\
          });\
        });\
        const { port } = await server.addr;\
        const res = await fetch(`http://127.0.0.1:${port}/`, { method: 'POST', body: 'bun' });\
        console.log('HTTP:' + res.status + ':' + res.headers.get('x-greeting') + ':' + (await res.text()));\
        await server.stop();";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("HTTP:201:hi:hello bun"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn unknown_runtime_builtin_module_errors() {
    let out = esrun()
        .arg("-e=import 'runtime:nope';")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn types_command_emits_declarations() {
    let out = esrun().arg("types").output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for m in ["runtime:process", "runtime:path", "runtime:fs"] {
        assert!(
            s.contains(&format!("declare module \"{m}\"")),
            "missing declaration for {m} in:\n{s}"
        );
    }
}

#[test]
fn types_install_writes_package_and_wires_tsconfig() {
    let dir = std::env::temp_dir().join(format!("esrun-types-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let out = esrun()
        .arg("types")
        .arg("--install")
        .current_dir(&dir)
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    // A type package is written under node_modules/@opentf/esrun.
    let dts = dir.join("node_modules/@opentf/esrun/index.d.ts");
    assert!(dts.exists(), "index.d.ts not written");
    assert!(dir.join("node_modules/@opentf/esrun/package.json").exists());
    assert!(
        std::fs::read_to_string(&dts)
            .unwrap()
            .contains("declare module \"runtime:fs\"")
    );

    // tsconfig.json is created and wired up (typeRoots + types).
    let ts = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
    assert!(
        ts.contains("node_modules/@opentf"),
        "typeRoots missing:\n{ts}"
    );
    assert!(ts.contains("\"esrun\""), "types entry missing:\n{ts}");

    // Re-running is idempotent — `esrun` isn't duplicated in `types`.
    let out2 = esrun()
        .arg("types")
        .arg("--install")
        .current_dir(&dir)
        .output()
        .expect("spawn esrun");
    assert!(out2.status.success());
    let ts2 = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
    assert_eq!(
        ts2.matches("\"esrun\"").count(),
        1,
        "esrun duplicated:\n{ts2}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_flag_succeeds() {
    let out = esrun().arg("--version").output().expect("spawn esrun");
    assert!(out.status.success());
    assert!(stdout(&out).contains("esrun"), "{}", stdout(&out));
}

#[test]
fn unhandled_rejection_reports_stack_trace() {
    let out = esrun()
        .arg("-e=setTimeout(() => { Promise.reject(new TypeError('async boom')); }, 0);")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "should exit non-zero");
    let stderr = stderr(&out);
    // Printed where it happened, not collected for the end — the summary that
    // follows is only the exit status, since repeating the message would report
    // one failure twice.
    assert!(
        stderr.contains("error: unhandled promise rejection"),
        "{stderr}"
    );
    assert!(stderr.contains("TypeError: async boom"), "{stderr}");
    assert!(stderr.contains("at file://"), "{stderr}");
    assert!(
        stderr.contains("1 unhandled failure — reported above"),
        "{stderr}"
    );
}

#[test]
fn runtime_urlpattern_works_globally() {
    let script = "
        // Test 1: Basic string pattern with base
        const p1 = new URLPattern('/api/users/:id', 'https://api.example.com');
        console.log('MATCH1=' + p1.test('https://api.example.com/api/users/123'));
        console.log('MATCH2=' + p1.test('https://api.example.com/api/posts/123'));
        console.log('ID1=' + p1.exec('https://api.example.com/api/users/456').pathname.groups.id);

        // Test 2: Absolute pattern string
        const p2 = new URLPattern('https://api.example.com/api/users/:id');
        console.log('MATCH3=' + p2.test('https://api.example.com/api/users/123'));

        // Test 3: Object pattern with wildcards
        const p3 = new URLPattern({ protocol: 'http*', hostname: '*.example.com', pathname: '/data/*' });
        console.log('MATCH4=' + p3.test('https://sub.example.com/data/123/456'));
        console.log('MATCH5=' + p3.test('ftp://sub.example.com/data/123'));

        // Test 4: Parameter mapping in different parts
        const p4 = new URLPattern({ hostname: ':sub.example.com', pathname: '/files/:file' });
        const exec4 = p4.exec('https://test.example.com/files/document.txt');
        console.log('SUB=' + exec4.hostname.groups.sub);
        console.log('FILE=' + exec4.pathname.groups.file);

        // Test 5: Ignored case. A dictionary carries its own baseURL — pairing
        // one with a separate base argument is a TypeError.
        const p5 = new URLPattern(
          { pathname: '/API/:id', baseURL: 'https://api.example.com' },
          { ignoreCase: true },
        );
        console.log('MATCH6=' + p5.test('https://api.example.com/api/123'));
    ";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("MATCH1=true"), "{}", s);
    assert!(s.contains("MATCH2=false"), "{}", s);
    assert!(s.contains("ID1=456"), "{}", s);
    assert!(s.contains("MATCH3=true"), "{}", s);
    assert!(s.contains("MATCH4=true"), "{}", s);
    assert!(s.contains("MATCH5=false"), "{}", s);
    assert!(s.contains("SUB=test"), "{}", s);
    assert!(s.contains("FILE=document.txt"), "{}", s);
    assert!(s.contains("MATCH6=true"), "{}", s);
}

#[test]
fn import_meta_resolve_resolves_against_the_module() {
    let out = run_file("resolve.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);

    assert!(stdout.contains("TYPE:function"), "{stdout}");
    // Relative and parent specifiers resolve against import.meta.url, so the
    // result is an absolute file: URL naming the sibling / parent file.
    assert!(stdout.contains("REL:file://"), "{stdout}");
    assert!(stdout.contains("/fixtures/greet.mjs"), "{stdout}");
    assert!(stdout.contains("PARENT:file://"), "{stdout}");
    assert!(
        stdout.contains("/up.mjs") && !stdout.contains("/fixtures/up.mjs"),
        "`../` must climb out of the fixtures directory: {stdout}"
    );
    // An absolute POSIX path resolves against the *root of the base URL*, which
    // on Windows includes the drive letter: `file:///D:/abs/z.mjs`. That is
    // WHATWG resolution, and what Node prints there too — so assert the shape
    // rather than a path only Unix produces.
    assert!(
        stdout.contains("ABS:file:///") && stdout.contains("/abs/z.mjs"),
        "{stdout}"
    );
    assert!(stdout.contains("URL:file:///q.mjs"), "{stdout}");
    // A `runtime:` builtin is already absolute and resolves to itself.
    assert!(stdout.contains("BUILTIN:runtime:process"), "{stdout}");
    // Resolution does no I/O: a path that does not exist still resolves.
    assert!(stdout.contains("MISSING:file://"), "{stdout}");
    assert!(stdout.contains("/definitely-not-here.mjs"), "{stdout}");
    // A bare specifier would need node_modules read synchronously; rather than
    // answer with a URL it never resolved, it refuses.
    // A bare specifier resolves through the loader (D41), and the URL it gives
    // back is one import() accepts — the same module instance, not a lookalike.
    assert!(stdout.contains("PKG:file://"), "{stdout}");
    assert!(
        stdout.contains("/node_modules/greeter/index.mjs"),
        "{stdout}"
    );
    assert!(stdout.contains("PARITY:true"), "{stdout}");
    // A #private specifier resolves through the package's own "imports" map.
    assert!(stdout.contains("PRIV:file://"), "{stdout}");
    assert!(stdout.contains("/esm/exporter.mjs"), "{stdout}");
    // What cannot be resolved throws, naming what it looked for.
    assert!(stdout.contains("MISSINGPKG!TypeError"), "{stdout}");
    assert!(stdout.contains("PRIVMSG:true"), "{stdout}");
    assert!(stdout.contains("NODE!TypeError"), "{stdout}");
}

#[test]
fn serve_rejects_incomplete_tls_options() {
    // Failing at `serve` beats binding a port and then rejecting every
    // handshake, which looks like a working server nothing can talk to.
    let out = esrun()
        .arg(format!(
            "-e={}",
            "import { serve } from 'runtime:http'; \
             const cases = [ \
               { secureTransport: 'on' }, \
               { secureTransport: 'on', cert: 'x' }, \
               { secureTransport: 'on', key: 'x' }, \
               { secureTransport: 'yes', cert: 'x', key: 'x' }, \
             ]; \
             for (const o of cases) { \
               try { serve({ port: 0, ...o }, () => new Response('x')); console.log('NO THROW'); } \
               catch (e) { console.log(`THREW ${e.constructor.name}`); } \
             } \
             console.log('TLS_OPTS_OK');",
        ))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert_eq!(
        stdout.matches("THREW TypeError").count(),
        4,
        "every incomplete or unknown TLS option must throw: {stdout}"
    );
    assert!(!stdout.contains("NO THROW"), "{stdout}");
    assert!(stdout.contains("TLS_OPTS_OK"), "{stdout}");
}

#[test]
fn the_added_fs_surface_works_against_a_real_disk() {
    // The in-process tests use an in-memory filesystem, so this is the one that
    // exercises the real jail, real temp-name generation, and real permissions.
    let out = run_file("fs-surface.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("TEMPDIR named:true"), "{stdout}");
    assert!(stdout.contains("TEMPDIR unique:true"), "{stdout}");
    assert!(stdout.contains("TEMPFILE inside:true"), "{stdout}");
    assert!(stdout.contains("COPY bytes:11 same:true"), "{stdout}");
    assert!(stdout.contains("TRUNCATE text:hello"), "{stdout}");
    assert!(stdout.contains("REALPATH clean:true"), "{stdout}");
    assert!(stdout.contains("CHMOD ok:true"), "{stdout}");
    assert!(stdout.contains("WRITE readable-in-full:true"), "{stdout}");
    assert!(
        stdout.contains("REALPATH missing:ERR_NOT_FOUND"),
        "{stdout}"
    );
    assert!(stdout.contains("FS_SURFACE_OK"), "{stdout}");
}
