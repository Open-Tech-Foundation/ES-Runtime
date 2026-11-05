//! `dev.servers` end to end (DECISIONS D208): several servers under one
//! `esdev start`, esdev as the front door — files, and a proxy rule that names
//! a server — and each server's errors on the page under its own name.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

const API: &str = r#"import { serve } from "runtime:http";
import { env } from "runtime:process";
serve({ hostname: "127.0.0.1", port: Number(env.PORT) }, (request) =>
  new Response(`api saw ${new URL(request.url).pathname} for ${request.headers.get("host")} as ${env.ROLE}`));
"#;

/// A worker that binds nothing and fails a job once the page is listening.
const WORKER: &str = r#"import { env } from "runtime:process";
setInterval(() => {}, 1000);
setTimeout(() => reportError(new Error(`job failed in ${env.ROLE}`)), Number(env.FAIL_AFTER));
"#;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esdev-servers-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the fixture");
    std::fs::write(dir.join("src/api.mjs"), API).expect("write the api");
    std::fs::write(dir.join("src/worker.mjs"), WORKER).expect("write the worker");
    std::fs::write(dir.join("src/main.mjs"), "document.title = 'spa';\n").expect("write the page");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><title>front door</title>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head><body></body></html>\n",
    )
    .expect("write the document");
    std::fs::write(
        dir.join("esdev.json"),
        r#"{ "build": { "targets": {
              "web": { "entry": "index.html", "outdir": "dist/web" },
              "api": { "entry": "src/api.mjs", "out": "dist/api.js" },
              "jobs": { "entry": "src/worker.mjs", "out": "dist/jobs.js" } } },
            "dev": {
              "servers": {
                "api": { "env": { "ROLE": "public" } },
                "worker": { "target": "jobs", "after": ["api"],
                            "env": { "ROLE": "background", "FAIL_AFTER": "1500" } } },
              "server": { "proxy": { "/api": "api" } } } }"#,
    )
    .expect("write the config");
    dir
}

struct Loop {
    child: Child,
    dir: PathBuf,
    log: PathBuf,
}

impl Drop for Loop {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return String::new();
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    );
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

#[tokio::test]
async fn esdev_is_the_front_door_for_several_servers() {
    let dir = project("front");
    let log = dir.join("loop.log");
    let child = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .arg("start")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).expect("create the log"))
        .spawn()
        .expect("spawn esdev start");
    let dev = Loop { child, dir, log };
    let read_log = || std::fs::read_to_string(&dev.log).unwrap_or_default();

    let deadline = Instant::now() + Duration::from_secs(120);
    let front = loop {
        let text = read_log();
        if let Some(at) = text.find("the app is on http://localhost:") {
            let port: u16 = text[at + "the app is on http://localhost:".len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .expect("a port");
            break port;
        }
        assert!(Instant::now() < deadline, "no banner:\n{}", read_log());
        std::thread::sleep(Duration::from_millis(100));
    };

    // The update channel, on the same origin.
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", front))
        .await
        .expect("connect to the front door");
    let (mut socket, _) =
        tokio_tungstenite::client_async(format!("ws://127.0.0.1:{front}/@esdev/hmr"), stream)
            .await
            .expect("the update channel");

    // Files from esdev, the API through the rule that names it, with the
    // page's Host kept and the server's own env.
    let mut api = String::new();
    while Instant::now() < deadline {
        api = get(front, "/api/hello");
        if api.contains("200 OK") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        api.contains(&format!(
            "api saw /api/hello for localhost:{front} as public"
        )),
        "{api}\n{}",
        read_log()
    );
    assert!(get(front, "/").contains("front door"), "{}", read_log());

    // The worker's failure, on the page, under its name.
    let frame = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(Ok(message)) = socket.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message
                && text.contains("\"type\":\"error\"")
            {
                return text.to_string();
            }
        }
        String::new()
    })
    .await
    .expect("no worker error reached the page");
    assert!(
        frame.contains("\"title\":\"Server error: worker\""),
        "{frame}"
    );
    assert!(frame.contains("job failed in background"), "{frame}");
}
