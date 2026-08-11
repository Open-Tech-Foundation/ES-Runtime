# Security

ES-Runtime is a security-hardened, embeddable JavaScript runtime (Layer A). This
document records the project's security posture and any **known, accepted gaps**
that are tracked for revisit. Architectural guarantees are specified in
`docs/SPEC.md` §4 and the rationale in `docs/DECISIONS.md`. A full threat model,
attack-surface enumeration, and residual-risk register is in
[`docs/SECURITY-REVIEW.md`](docs/SECURITY-REVIEW.md).

## Reporting

Until a formal channel is published, report suspected vulnerabilities privately
to the maintainer rather than via public issues.

## Runtime safety status

The resource-limit / FFI-safety spine (SPEC.md §4) is in place as of Phase 9:

- **Heap limit** — a near-limit guard terminates execution before the host OOMs.
- **Execution watchdog** — a thread-safe `InterruptHandle` terminates a runaway
  script; it surfaces as `Error::Terminated`, never a hang (`esrun --timeout`).
- **Stack guard** — V8-native; deep recursion is a catchable `RangeError`.
- **Bounded pending-ops** — adversarial JS can't pile up unbounded host work.
- **Panic containment** — op/timer/reject callbacks are `catch_unwind`-wrapped,
  so a host panic is a JS exception, not an unwind across the FFI (assumes
  `panic = "unwind"`).
- **Deny-by-default capabilities**; deterministic providers for reproducibility.

**Not yet hardened** (later Phase 9): `cargo-fuzz` (URL/streams/encoding/the
marshaler), sanitizer CI (Miri/ASAN), a WPT/min-common conformance run, a
systematic intrinsic-integrity (prototype-pollution) audit, and an external
security review. **Until those land, do not run hostile/untrusted code** with
`esrun`, `--deny-all` or not; the embeddable library lets an embedder restrict
capabilities and inject its own providers.

## Restricting an `esrun` run (`--deny-all` / `--deny-<name>`)

`esrun` grants every capability by default — it runs a local script the user
named. Restriction is opt-in (DECISIONS D38), and there are exactly two modes,
which **cannot be combined**:

```sh
esrun --deny-net --deny-run app.js                     # everything, minus these
esrun --deny-all --allow-imports --allow-net app.js    # nothing, plus these
```

| Mode | Baseline | Direction |
|---|---|---|
| `--deny-<name>` | everything granted | subtractive only |
| `--deny-all --allow-<name>` | nothing granted | additive only |

`--allow-<name>` requires `--deny-all`: with everything already granted there is
nothing for it to add. Because neither mode mixes directions, **no flag ever
overrides another** — a reader goes top to bottom and the list is the answer.

The eight names are `read`, `write`, `imports`, `net`, `listen`, `env`, `run`,
`signals` — the same words the `runtime:process` `permissions` API and the
denial message use. A denied operation throws `NotAllowedError`
(`ERR_CAPABILITY_DENIED`) **before** the effect, never a partial one.

The parser is strict on purpose: a space-separated value, a permission
flag placed after the script, or an unknown name is an **error**. Each of those, ignored,
would leave a run wider than the command line claims.

Two things `--deny-all` does *not* do, both deliberate:

- **It still runs the entry file.** That file is read by the CLI before a
  runtime exists, so it is outside the capability system — the user named it,
  and the flags govern what it may then do. Since `--deny-all` includes
  `--deny-imports`, a fully denied run is a **single-file** run; add
  `--allow-imports` for an application with dependencies.
- **It does not revoke `Clock`/`Entropy`/`Timers`/`TaskSpawn`.** No op gates
  them; a denied script still computes, it just reaches nothing.

Importing a `runtime:` module always works, under any policy — the gate is the
op, not the import. A program can ask what it is allowed to do:

```js
import { permissions } from "runtime:process";
permissions.denied;          // ["read", "write", ...]
permissions.has("net");      // false
```

**Seven of the eight can be scoped to a list.** (`imports` is the exception —
what may be *loaded* has its own mechanism, below.)

```sh
esrun --deny-all --allow-imports --allow-env=PORT,DATABASE_URL \
      --allow-net=db.internal:5432 --allow-listen=8080 \
      --allow-read=./data --allow-write=./out --allow-run=git \
      --allow-signals=SIGTERM server.js
```

- `--allow-net=<hosts>` refuses every other address, **on every redirect hop**
  as well as on the request the program wrote — the check that stops a
  compromised dependency exfiltrating over the app's own legitimate network
  access. Hosts are matched as written, before resolution, and exactly:
  `example.com` does not admit `api.example.com`, and there are no wildcards.
- `--allow-listen=<addresses>` refuses every other bind, before the port is
  claimed. A separate list from `net`: reaching out and being reachable are
  separate capabilities.
- `--allow-env=<names>` narrows the environment to those variables — every other
  one is *absent*, so a guest can neither read it nor learn its name.
- `--allow-run=<programs>` refuses to spawn anything else.
- `--allow-read=<paths>` / `--allow-write=<paths>` refuse every other path.
  Entries are resolved against the working directory and cover their subtree,
  and the check runs **after canonicalization**, so a symlink inside an allowed
  directory cannot name a file outside it. The two are separate lists, and both
  govern `runtime:fs` and `runtime:wasi` alike.
- `--allow-signals=<names>` refuses to watch anything else, and hides the rest
  from `signals()`. A watch suppresses the default action, so this is the
  privilege to decline to die on request, granted one signal at a time.

A refusal is `ERR_PERMISSION_DENIED` — a scoped denial, distinct from the
`ERR_CAPABILITY_DENIED` a missing capability raises. A scoped grant still
reports `permissions.has("net") === true`: the capability opens the door, the
list is what the provider withholds.

A value on a flag that could not enforce it would still be rejected rather than
ignored — the rule outlives the capabilities it was written for. The filesystem
**root jail** (D25) is always on regardless, for both paths: a path list narrows
it and never widens it, so an entry outside the project root is not a way out of
it.

## Import policy (what may be loaded)

Capabilities bound what running code may *reach*. What may **become** running
code is a separate question, and has a separate mechanism (DECISIONS D39) — a
JSON file, named explicitly and never auto-discovered:

```sh
esrun --deny-all --allow-imports --allow-net=db.internal:5432 \
      --import-policy=./import-policy.json server.js
```

```json
{ "allow": ["./src", "express", "@acme/ui"], "deny": ["aws-sdk"] }
```

An entry beginning with `.` or `/` is a path covering its subtree; anything else
is a package name — the split the loader already makes between a relative and a
bare specifier. **Deny wins over allow.** Omitting `"allow"` permits everything
not denied; an empty `"allow": []` is an error, not a run that can load nothing.
Unknown keys are an error, for the same reason an unknown flag is: a misspelled
`"allowed"` would read as protection that is not there. Paths resolve relative
to the **policy file**, so a committed policy means the same thing wherever the
run is invoked from.

Matching runs on the **resolved, canonicalized** module, after the root jail, so
a symlink cannot name its way in and a pnpm store path is still recognisably its
package. A package entry covers that package's own files and **not** the
packages it imports — each is named in its own right, so a dependency that
quietly pulls in another cannot load it. The entry file is exempt: it is read
before a loader exists.

The two layers do not substitute for each other. The `imports` capability
decides whether the loader runs at all; the policy decides what it may resolve.
A policy is **not a way around `--deny-imports`** — under `--deny-all`, an allow
entry still loads nothing.

**Known gap: no integrity.** A policy names packages and paths, not content.
`"express"` says the loader may resolve that package; it says nothing about
which version, or whether the bytes are the ones you audited. Lockfiles remain
the install-time counterpart; content pinning is future work. Treat the policy
as a bound on *which* dependencies can run, not as proof of *what* they are.

## Child processes (`Capability::Run`)

Spawning is the one grant that **ends the sandbox**, and it is treated as such
(DECISIONS D37):

- **`Run` is never implied** by another capability. A child process runs outside
  every confinement here — no capability check, no filesystem root jail, no
  execution watchdog reaches it. Granting `Run` to guest code is granting
  everything the host user can do. Withhold it from anything untrusted.
- **No shell.** `runtime:system` has no `exec`, no `shell: true`, and no
  template form. A command is a program plus an argv, so a guest-supplied
  argument reaches the child as data and can never become a second command. On
  Windows, `.bat`/`.cmd` files are refused rather than run through the command
  interpreter (CVE-2024-27980).
- **No inherited environment.** A child gets exactly the `env` it is passed.
  Inheriting is opt-in (`inheritEnv: true`) and **additionally requires `Env`**,
  so a runtime granted `Run` alone cannot launder the host's environment out
  through a child. A masked `Secret` is unwrapped only on its way into a child's
  environment, never into a log.
- **Policy belongs to the provider.** An embedder that must grant `Run` can
  still bound it: `SystemCommands::with_allowlist(["git", "ffmpeg"])` and
  `with_max_children(n)` are enforced in Rust, below the capability check.
- **No orphans.** Children still running when the runtime is torn down are
  killed, not reparented.

**Not covered:** killing a process *tree*. `kill()` signals the direct child
only, so a child that spawns its own children can leave grandchildren running.

## Workers (`Capability::Worker`)

A worker is a second agent: its own thread, its own V8 isolate, no shared heap.
Starting one is capability-gated, and everything it receives **narrows** from
the agent that started it (DECISIONS D48/D49):

- **A worker starts with nothing.** `new Worker(url)` grants no capability at
  all; each is named at the spawn — `{ permissions: ["net"] }` — and can never
  exceed what the spawning agent holds. `{ permissions: "inherit" }` asks for
  that agent's whole set and is still bounded by it. Nesting re-applies the rule
  at every level, so no chain of spawns widens the original grant. This is
  stricter than Deno, which clones the parent's permissions unmodified.
- **Two grants are needed to spawn at all**, not one: `workers`, and `imports`
  to read the worker's entry module. `--deny-all --allow-workers` alone is
  refused. Node requires `--allow-fs-read` alongside `--allow-worker` for the
  same reason; Deno requires `--allow-read`.
- **The entry module is read under the parent's authority, before the worker
  exists**, and the capability set narrows to the worker's own before a line of
  it runs. That is safe for one specific reason: **instantiation runs no guest
  code**. Dynamic `import()` inside a worker is a different matter — it picks
  its specifier at runtime, so it reads *and executes* a file chosen while the
  worker runs, and needs `imports` granted at the spawn.
- **An unknown permission name throws** rather than being skipped. Dropping it
  fails closed, which is quiet in the worst way: the worker takes the degraded
  path forever and the denial surfaces far from the typo.
- **The environment is attenuated, not inherited.** `{ env: { … } }` passes
  precisely those variables and needs no `env` capability, because a parent can
  only pass values it could already read. Node's `SHARE_ENV` has no equivalent
  here: a shared, mutable environment is an undeclared side channel between
  agents, and `postMessage` is the declared one.
- **Memory is bounded per agent.** A worker inherits the heap ceiling of the
  agent that started it (`--max-heap=<mb>`, by default sized from the container
  limit or host memory) and `{ memory: 64 }` may only lower it. Reaching it ends
  that worker alone, reported to the parent as `ERR_WORKER_OUT_OF_MEMORY`.
- **`terminate()` is real.** It interrupts the isolate, so it stops a worker
  spinning in a synchronous loop or parked in `Atomics.wait`, and it takes that
  worker's own workers with it — a nested worker left running would be
  unreachable and would still hold the process open.
- **Process control is not delegated.** `onSignal` is refused inside a worker,
  and `exit()` there ends that worker without setting the process's exit code.

**Not covered:** CPU time per worker — the execution watchdog (`--timeout`) is
per process, not per agent. Message queues between agents are unbounded, so a
producer that outruns its worker grows memory; `worker.queued` is the signal to
pace against, and it is advisory. `SharedArrayBuffer` is genuinely shared
memory: two agents holding one are not isolated from each other's writes, which
is what makes `Atomics` work and what makes it a channel a capability check
cannot see.

## Environment files & secret masking

`.env` support (DECISIONS D30) is built so that **what the guest can read from
the environment is an explicit host decision**, and so that secret values resist
**accidental** disclosure:

- **No implicit disk reads.** A single `.env` file is loaded **only** via an
  explicit `esrun --env-file=<path>`. There is no auto-discovery of a `.env`
  in the working directory or project root — nothing on disk is read into the
  guest's environment unless you ask for it. This is a CLI/host feature; the
  embeddable library never loads env files and never mutates the real process
  environment (the file values are an in-memory overlay on `runtime:process`).
- **OS environment wins by default.** Loaded values fill only keys the OS does
  not already set, so a checked-in `.env` cannot silently clobber a production
  deployment's real configuration. `--env-override` opts into letting file
  values win.
- **Secret masking.** Env entries with a secret-bearing key (case-insensitive)
  — ending in `_KEY(S)`, `_TOKEN(S)`, `_SECRET(S)`, `_PASS`, `_PASSWORD(S)`, or
  containing `CREDENTIAL`/`AUTH` (as an underscore-delimited word) — are exposed
  by `runtime:process` as an opaque
  `Secret` that renders as `"[redacted]"` in `console` output, string coercion /
  template literals, and `JSON.stringify`. The real value is held in a
  module-private `WeakMap` and is obtainable only via the explicit
  `unmask(value)` helper. **Scope:** this defends against *accidental* leakage
  to logs and serialized output — it is **not** a barrier against hostile guest
  code, which can call `unmask` itself (the guest is already trusted with the
  value). Parser errors never include a variable's value.

## Hashing & passwords (`runtime:hashing`)

`runtime:hashing` (DECISIONS D57) carries **no capability**: hashing reads
nothing and reaches nothing, so what it tells a caller is a fact about data they
already hold. Three things about it are security-relevant.

- **A salt is randomness, and randomness is granted.** `password.hash()` draws
  its salt in JavaScript from `crypto.getRandomValues`, so it needs `Entropy`
  like every other random byte here — no op helps itself to entropy.
  `password.verify()` needs **nothing**: the salt is inside the stored string,
  so a service that only checks passwords is granted nothing at all.
- **The parameters live in the stored hash, not in the configuration.**
  Verification reads the algorithm, cost and salt from the string it is given,
  so raising a default never invalidates existing hashes. `needsRehash()` is how
  they are replaced, at the one moment the plaintext is in hand.
- **bcrypt refuses what it would truncate.** bcrypt hashes at most 72 bytes
  including its own NUL, and most implementations silently ignore the rest —
  quietly making two different passwords the same password. Hashing past 71
  bytes is an error here. **Verification still truncates**, deliberately: a
  stored hash may have been written by one of those implementations, and
  verifying it must compute what *that* implementation computed.

`md5` and `sha1` are available for interop (S3 ETags, CRAM-MD5, legacy
protocols) and are broken for signatures; `xxhash*` and `crc32*` are checksums,
are not collision-resistant against an adversary, and are refused by `hmac`.
Password hashing runs on the calling thread and blocks it for the duration — a
public login endpoint wants a queue in front of it, since concurrent calls
compete for the same isolate.

## Intrinsic integrity (prototype pollution / global tampering)

**The security boundary is in Rust, not in JavaScript.** The op table and the
capability set live in the engine's `OpState`; every capability-gated op is
checked there before dispatch. Consequently, no amount of guest JS tampering —
polluting `Object`/`Array.prototype`, reassigning or deleting globals, or trying
to forge `globalThis.__ops` — can grant a capability or dispatch an op the host
did not register and gate. This is covered by tests (`capability_gate_survives_js_tampering`,
`op_table_binding_is_locked`, `op_dispatch_survives_prototype_pollution`).

As defense-in-depth on the JS surface, `harden.js` (the last prelude fragment)
locks the `globalThis.__ops` binding (non-writable/non-configurable, while the
object stays extensible so the host can still register ops) and freezes the
runtime's plain namespace objects (`console`; `crypto`/`performance` are frozen
at definition).

**Deliberately deferred — SES-style primordial hardening.** Freezing the JS
primordials (`Object.prototype`, `Array.prototype`, …) would protect the
*prelude's own* correctness against pollution, but it is an opinionated policy
with real guest-compatibility cost. It is left to the embedder / Layer B rather
than baked into a general-purpose Layer A. Until an embedder opts in, a guest
that pollutes primordials can break the *prelude's* JS behaviour for itself — it
still cannot escalate privilege past the Rust boundary.

## Supply-chain gates

Every change must pass `cargo deny check` and `cargo audit` in CI (`docs/SPEC.md`
§5). Advisory exceptions are never silenced globally: each is listed explicitly,
with a rationale, in **both** `deny.toml` and `.cargo/audit.toml`, and is
revisited rather than forgotten.

## Known accepted gaps (revisit list)

### RSA timing sidechannel — RUSTSEC-2023-0071 ("Marvin Attack")

- **What.** `crypto.subtle` RSA (RSASSA-PKCS1-v1_5, RSA-PSS, RSA-OAEP) is backed
  by the RustCrypto `rsa` crate (`docs/DECISIONS.md` D9). That crate carries
  RUSTSEC-2023-0071, a medium-severity (5.9) timing sidechannel in RSA
  private-key operations. **No fixed upgrade exists** — the issue is
  architectural in RustCrypto's RSA and has been open since 2023.
- **Why accepted (maintainer, 2026-06-12).** RSA private-key operations run
  **host-side**; a sandboxed guest does not get a high-resolution local timing
  oracle against them, which lowers practical exploitability. The alternatives
  were weighed and each costs more than it buys for this project:
  - **aws-lc-rs** (constant-time) draws randomness from its own internal OS
    CSPRNG with no hook for the injected `Entropy` provider — breaking the
    runtime's "no ambient authority / all I/O injected" thesis for RSA — and
    adds a C/assembly crypto backend to the otherwise pure-Rust `runtime` crate.
  - **openssl-rs** adds a system OpenSSL dependency, regressing the portable,
    self-contained build goal (SPEC §1, D2).
- **Mitigations in place.** All RSA randomness (key generation, PSS salt,
  PKCS#1 v1.5 blinding, OAEP padding) is routed through the injected `Entropy`
  provider — never ambient `OsRng` — preserving determinism under seeded
  providers and capability control. RSA is capability-gated like all of
  `crypto.subtle`.
- **Revisit when.** RustCrypto ships a constant-time RSA, or the
  `elliptic-curve` 0.14 / `digest` 0.11 generation reshapes the stack such that
  a vetted, constant-time, provider-routable backend becomes available.

### `paste` unmaintained — RUSTSEC-2024-0436

Informational (unmaintained, not a vulnerability). Reaches us only transitively
through the `v8` crate; not a direct dependency and not removable without an
upstream v8 change. Revisit when v8 drops it.
