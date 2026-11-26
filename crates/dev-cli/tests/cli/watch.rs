//! `--watch` (D59).

use super::*;

#[test]
fn watch_reruns_the_program_when_a_file_changes() {
    let dir = watch_dir("w_rerun");
    // A `.txt` sink: not a watched extension, so the program's own output
    // cannot be what triggers the next run.
    let sink = dir.join("runs.txt");
    let app = dir.join("app.mjs");
    let program = |marker: &str| {
        format!(
            "import {{ write }} from 'runtime:fs';\n\
             await write({:?}, '{marker}\\n', {{ append: true }});\n",
            sink.to_string_lossy()
        )
    };
    std::fs::write(&app, program("FIRST")).expect("write app");

    let mut child = esdev_in(&dir)
        .args(["--watch", "app.mjs"])
        .spawn()
        .expect("spawn esdev --watch");

    let first = wait_for_file(&sink, Duration::from_secs(20), |s| s.contains("FIRST"));
    assert!(
        first.contains("FIRST"),
        "first run never happened: {first:?}"
    );

    std::fs::write(&app, program("SECOND")).expect("rewrite app");
    let both = wait_for_file(&sink, Duration::from_secs(20), |s| s.contains("SECOND"));

    let _ = child.kill();
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
    assert!(both.contains("FIRST"), "{both:?}");
    assert!(
        both.contains("SECOND"),
        "the change did not rerun it: {both:?}"
    );
}

/// The regression that matters: `inotify` reports reads, so the child *loading*
/// its entry raises an event on a watched file. A watcher that treats that as a
/// change restarts forever with nobody touching anything.
#[test]
fn watch_does_not_restart_because_it_restarted() {
    let dir = watch_dir("w_no_loop");
    let sink = dir.join("runs.txt");
    let app = dir.join("app.mjs");
    std::fs::write(
        &app,
        format!(
            "import {{ write }} from 'runtime:fs';\n\
             await write({:?}, 'x', {{ append: true }});\n",
            sink.to_string_lossy()
        ),
    )
    .expect("write app");

    let mut child = esdev_in(&dir)
        .args(["--watch", "app.mjs"])
        .spawn()
        .expect("spawn esdev --watch");

    // One run, then nothing — no edits are made after this point.
    wait_for_file(&sink, Duration::from_secs(20), |s| !s.is_empty());
    std::thread::sleep(Duration::from_secs(3));
    let settled = std::fs::read_to_string(&sink).unwrap_or_default();

    let _ = child.kill();
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        settled.len(),
        1,
        "ran {} times with no edit — the watcher is retriggering itself",
        settled.len()
    );
}

/// The watcher narrates itself: a banner naming what is watched, then one
/// timed line per restart.
#[test]
fn watch_narrates_startup_and_restarts() {
    use std::io::BufRead as _;

    let dir = watch_dir("w_narrates");
    let app = dir.join("app.mjs");
    std::fs::write(&app, "console.log('once');\n").expect("write app");

    let mut child = esdev_in(&dir)
        .args(["--watch", "app.mjs"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn esdev --watch");
    let stderr = child.stderr.take().expect("stderr");
    let mut lines = std::io::BufReader::new(stderr).lines();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);

    // The banner, then the first run finishing — only with the supervisor
    // parked waiting for a change is a rewrite guaranteed to wake it: a
    // change that lands before the watcher starts is missed, like every
    // watcher misses the past.
    let mut saw_banner = false;
    let mut first_done = false;
    while std::time::Instant::now() < deadline {
        let Ok(Some(line)) = lines.next().transpose() else {
            break;
        };
        saw_banner |= line.contains("— watching .");
        first_done |= line.contains("program exited");
        if saw_banner && first_done {
            break;
        }
    }
    assert!(saw_banner, "no watch banner");
    assert!(first_done, "the first run never finished");

    std::fs::write(&app, "console.log('twice');\n").expect("rewrite app");
    let mut saw_restart = false;
    while std::time::Instant::now() < deadline {
        let Ok(Some(line)) = lines.next().transpose() else {
            break;
        };
        if line.contains("restarted in") {
            saw_restart = true;
            break;
        }
    }

    let _ = child.kill();
    let _ = child.wait();

    let _ = std::fs::remove_dir_all(&dir);
    assert!(saw_restart, "no restart line for the save");
}

#[test]
fn watch_needs_a_file_to_watch() {
    let out = esdev()
        .args(["--watch", "-e=console.log(1)"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("needs a file to watch"),
        "{}",
        stderr(&out)
    );
}
