//! End-to-end tests for `--proxy` / `--no-proxy` (DECISIONS D205).
//!
//! A forwarding proxy and an origin run on loopback threads. The target is
//! named `api.test`, which resolves nowhere: reaching it at all proves the
//! request went through the proxy, which maps `*.test` to loopback.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

mod common;

/// An origin answering every request with `origin says hi`.
fn origin() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && line != "\r\n" {
                    line.clear();
                }
                let body = "origin says hi";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    port
}

/// A forwarding proxy: reads one request head, records its request line,
/// sends it to the absolute-form target (`*.test` is loopback), and pipes
/// the answer back.
fn proxy() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let seen = lines.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut client) = stream else { return };
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if client.read(&mut byte).unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&head).into_owned();
                let request_line = text.lines().next().unwrap_or_default().to_string();
                seen.lock().unwrap().push(request_line.clone());
                let target = request_line.split_whitespace().nth(1).unwrap_or_default();
                let authority = target
                    .trim_start_matches("http://")
                    .split('/')
                    .next()
                    .unwrap_or_default();
                let port = authority.rsplit_once(':').map_or("80", |(_, p)| p);
                let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", port.parse().unwrap()))
                else {
                    return;
                };
                upstream.write_all(&head).unwrap();
                let mut answer = Vec::new();
                let _ = upstream.read_to_end(&mut answer);
                let _ = client.write_all(&answer);
            });
        }
    });
    (port, lines)
}

fn esrun(args: &[String], env: &[(&str, String)]) -> Output {
    let mut command = Command::new(common::esrun_bin());
    command.args(args);
    for var in [
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(var);
    }
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("failed to spawn esrun")
}

fn fetch_script(port: u16) -> String {
    format!(
        "-e=const r = await fetch('http://api.test:{port}/hello'); console.log(await r.text());"
    )
}

#[test]
fn fetch_goes_through_the_proxy_the_flag_names() {
    let port = origin();
    let (proxy_port, seen) = proxy();
    let out = esrun(
        &[
            "--allow-net".into(),
            format!("--proxy=http://127.0.0.1:{proxy_port}"),
            fetch_script(port),
        ],
        &[],
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout.trim(), "origin says hi");
    assert_eq!(
        *seen.lock().unwrap(),
        [format!("GET http://api.test:{port}/hello HTTP/1.1")]
    );
}

#[test]
fn the_environment_is_never_read_and_says_so() {
    let port = origin();
    let (proxy_port, seen) = proxy();
    let out = esrun(
        &["--allow-net".into(), fetch_script(port)],
        &[("HTTP_PROXY", format!("http://127.0.0.1:{proxy_port}"))],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "api.test resolved without the proxy");
    assert!(
        stderr.contains("HTTP_PROXY is ignored; esrun reads no proxy from the environment"),
        "{stderr}"
    );
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_no_proxy_entry_goes_direct() {
    let port = origin();
    let (proxy_port, seen) = proxy();
    let out = esrun(
        &[
            "--allow-net".into(),
            format!("--proxy=http://127.0.0.1:{proxy_port}"),
            "--no-proxy=.test".into(),
            fetch_script(port),
        ],
        &[],
    );
    assert!(!out.status.success(), "api.test resolved without the proxy");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn a_bad_proxy_url_stops_the_run_without_echoing_it() {
    let out = esrun(
        &["--proxy=socks5://me:hunter2@p:1080".into(), "-e=1".into()],
        &[],
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("must be http:// or https://"), "{stderr}");
    assert!(!stderr.contains("hunter2"), "{stderr}");
    let out = esrun(&["--no-proxy=a.com".into(), "-e=1".into()], &[]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--no-proxy needs --proxy"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
