//! Filesystem ops: capability gates and what each refuses.

use super::*;

/// A [`SyncFileSystem`] that records what it was asked to do and never
/// touches a real disk — enough to prove the *gate* is what stops a call,
/// rather than the filesystem happening to fail.
#[derive(Default)]
struct RecordingSyncFs {
    calls: Mutex<Vec<String>>,
}

impl es_runtime_providers::SyncFileSystem for RecordingSyncFs {
    fn open(
        &self,
        path: &str,
        _options: es_runtime_providers::SyncOpenOptions,
    ) -> std::result::Result<u32, es_runtime_providers::ProviderError> {
        self.calls.lock().unwrap().push(format!("open {path}"));
        Ok(1)
    }
    fn read(
        &self,
        _fd: u32,
        buf: &mut [u8],
    ) -> std::result::Result<usize, es_runtime_providers::ProviderError> {
        self.calls.lock().unwrap().push("read".into());
        let data = b"data";
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
    fn write(
        &self,
        _fd: u32,
        data: &[u8],
    ) -> std::result::Result<usize, es_runtime_providers::ProviderError> {
        self.calls.lock().unwrap().push("write".into());
        Ok(data.len())
    }
    fn seek(
        &self,
        _fd: u32,
        _offset: i64,
        _whence: es_runtime_providers::SyncWhence,
    ) -> std::result::Result<u64, es_runtime_providers::ProviderError> {
        Ok(0)
    }
    fn close(&self, _fd: u32) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        Ok(())
    }
    fn fstat(
        &self,
        _fd: u32,
    ) -> std::result::Result<es_runtime_providers::FileStat, es_runtime_providers::ProviderError>
    {
        Ok(es_runtime_providers::FileStat {
            size: 4,
            is_file: true,
            is_dir: false,
            is_symlink: false,
            mtime_ms: None,
        })
    }
    fn stat(
        &self,
        _path: &str,
    ) -> std::result::Result<es_runtime_providers::FileStat, es_runtime_providers::ProviderError>
    {
        Ok(es_runtime_providers::FileStat {
            size: 4,
            is_file: true,
            is_dir: false,
            is_symlink: false,
            mtime_ms: None,
        })
    }
    fn read_dir(
        &self,
        _path: &str,
    ) -> std::result::Result<Vec<es_runtime_providers::DirEntry>, es_runtime_providers::ProviderError>
    {
        Ok(Vec::new())
    }
    fn mkdir(&self, path: &str) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        self.calls.lock().unwrap().push(format!("mkdir {path}"));
        Ok(())
    }
    fn remove_file(
        &self,
        _path: &str,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        Ok(())
    }
    fn remove_dir(
        &self,
        _path: &str,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        Ok(())
    }
    fn rename(
        &self,
        _from: &str,
        _to: &str,
    ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
        Ok(())
    }
}

fn runtime_with_sync_fs(fs: Arc<RecordingSyncFs>) -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    Runtime::new(Box::new(engine), test_providers().with_sync_file_system(fs)).expect("runtime")
}

fn runtime_with_memory_fs() -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    Runtime::new(
        Box::new(engine),
        test_providers().with_file_system(Arc::new(MemoryFs::default())),
    )
    .expect("runtime")
}

/// A copy reads one path and writes another, so it must hold **both**
/// grants. Gating it on the write alone would let a guest with no read
/// access duplicate a file it cannot see into somewhere it can reach by
/// another route — an exfiltration primitive out of a write-only grant.
#[test]
fn copy_requires_both_file_capabilities() {
    let _g = v8_guard();
    let mut rt = runtime_with_memory_fs();
    rt.expose_ops_for_testing();

    rt.set_capabilities(CapabilitySet::none().with(Capability::FileWrite));
    assert!(
        rt.eval("__ops.fs_copy('/a', '/b')").is_err(),
        "FileWrite alone must not permit a copy"
    );

    rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
    assert!(
        rt.eval("__ops.fs_copy('/a', '/b')").is_err(),
        "FileRead alone must not permit a copy"
    );

    // Both together dispatch (and fail on the missing source, not the gate).
    rt.set_capabilities(
        CapabilitySet::none()
            .with(Capability::FileRead)
            .with(Capability::FileWrite),
    );
    let out = eval_async(
        &mut rt,
        "try { await __ops.fs_copy('/a', '/b'); return 'no throw'; } catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_NOT_FOUND".into()));
}

/// Copying a file onto itself has to be refused rather than performed: on the
/// real filesystem the destination is truncated before the source is read, so
/// it emptied the file and reported `0` bytes copied.
#[test]
fn copying_a_file_onto_itself_is_refused() {
    let _g = v8_guard();
    let mut rt = runtime_with_memory_fs();
    rt.expose_ops_for_testing();
    rt.set_capabilities(
        CapabilitySet::none()
            .with(Capability::FileRead)
            .with(Capability::FileWrite),
    );
    let out = eval_async(
        &mut rt,
        "await __ops.fs_write('/a', new TextEncoder().encode('payload'), false); \
         try { await __ops.fs_copy('/a', '/a'); return 'no throw'; } catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_SAME_FILE".into()));

    let survived = eval_async(
        &mut rt,
        "return new TextDecoder().decode(await __ops.fs_read('/a'));",
    );
    assert_eq!(survived, Value::String("payload".into()));
}

/// The read-side and write-side additions land on the gate that matches what
/// they do, rather than all defaulting to one.
#[test]
fn the_new_fs_ops_are_gated_by_what_they_do() {
    let _g = v8_guard();
    let mut rt = runtime_with_memory_fs();
    rt.expose_ops_for_testing();

    rt.set_capabilities(CapabilitySet::none().with(Capability::FileWrite));
    for read_op in ["__ops.fs_real_path('/a')", "__ops.fs_read_link('/a')"] {
        assert!(
            rt.eval(read_op).is_err(),
            "{read_op} must need FileRead, not FileWrite"
        );
    }

    rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
    for write_op in [
        "__ops.fs_truncate('/a', 0)",
        "__ops.fs_chmod('/a', 384)",
        // Creating a link stores a string and reads nothing; following one
        // later is a read, and gated where reads are.
        "__ops.fs_symlink('/a', '/b', null)",
        "__ops.fs_make_temp_dir('', 't-')",
        "__ops.fs_make_temp_file('', 't-')",
    ] {
        assert!(
            rt.eval(write_op).is_err(),
            "{write_op} must need FileWrite, not FileRead"
        );
    }
}

/// The synchronous filesystem is behind the same gates as the async one:
/// without `FileWrite`, a mutating op is denied before the provider is
/// consulted at all.
#[test]
fn sync_fs_ops_are_capability_gated() {
    let _g = v8_guard();
    let fs = Arc::new(RecordingSyncFs::default());
    let mut rt = runtime_with_sync_fs(fs.clone());
    rt.expose_ops_for_testing();

    // Deny-by-default: nothing granted, so even a read is refused.
    rt.set_capabilities(CapabilitySet::none());
    assert!(
        rt.eval("__ops.sync_fs_open('/x', { read: true })").is_err(),
        "a read open must need FileRead"
    );

    // FileRead alone permits reading but not mutating.
    rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
    rt.eval("__ops.sync_fs_open('/x', { read: true })")
        .expect("read open is permitted by FileRead");
    assert!(
        rt.eval("__ops.sync_fs_mkdir('/d')").is_err(),
        "mkdir must need FileWrite"
    );
    assert!(
        rt.eval("__ops.sync_fs_open_write('/x', { write: true })")
            .is_err(),
        "a write open must need FileWrite"
    );

    // The provider saw only the permitted call — the denials never reached it.
    let calls = fs.calls.lock().unwrap().clone();
    assert_eq!(calls, vec!["open /x".to_string()], "calls: {calls:?}");

    // Granting FileWrite opens the mutating paths.
    rt.set_capabilities(
        CapabilitySet::none()
            .with(Capability::FileRead)
            .with(Capability::FileWrite),
    );
    rt.eval("__ops.sync_fs_mkdir('/d')")
        .expect("mkdir is permitted by FileWrite");
}

/// The read-only open op refuses a mutating mode outright, so the
/// `FileWrite` gate on `sync_fs_open_write` cannot be sidestepped by asking
/// the `FileRead`-gated op to create a file.
#[test]
fn the_read_open_op_refuses_a_mutating_mode() {
    let _g = v8_guard();
    let fs = Arc::new(RecordingSyncFs::default());
    let mut rt = runtime_with_sync_fs(fs.clone());
    rt.expose_ops_for_testing();
    rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));

    for mode in [
        "{ write: true }",
        "{ create: true }",
        "{ truncate: true }",
        "{ createNew: true }",
    ] {
        let err = rt
            .eval(&format!("__ops.sync_fs_open('/x', {mode})"))
            .unwrap_err();
        assert!(
            format!("{err}").contains("sync_fs_open_write"),
            "mode {mode} should be redirected, got: {err}"
        );
    }
    assert!(
        fs.calls.lock().unwrap().is_empty(),
        "no mutating open should have reached the provider"
    );
}

/// With no synchronous filesystem installed, the ops fail cleanly rather
/// than panicking — which is what makes WASI report `ENOTCAPABLE`.
#[test]
fn sync_fs_ops_without_a_provider_fail_cleanly() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.set_capabilities(
        CapabilitySet::none()
            .with(Capability::FileRead)
            .with(Capability::FileWrite),
    );
    let err = rt
        .eval("__ops.sync_fs_open('/x', { read: true })")
        .unwrap_err();
    assert!(
        format!("{err}").contains("no synchronous filesystem"),
        "got: {err}"
    );
}
