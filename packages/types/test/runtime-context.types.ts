// A type test for `runtime:context`.
//
// Same reasoning as `runtime-test.types.ts`: these declarations describe a
// surface they do not implement, so the runtime's own suite would go on passing
// while the types beside it said something else. `@ts-expect-error` fails the
// build when the error it names *stops* happening, so a declaration that
// quietly widened to `any` breaks this file rather than passing it.
//
// The inference this file mostly exists to pin is `run()`'s: it must hand back
// the callback's own return type, including a promise, and it must type the
// trailing arguments against the callback's parameters. Getting that wrong is
// invisible at runtime and ruins the module in an editor.

import type { Context, ContextOptions, TaskInfo } from "runtime:context";
import { bind, createContext, currentTask, snapshot, withTrace } from "runtime:context";

// --- createContext ------------------------------------------------------------

const tenant = createContext<string>({ name: "tenant", defaultValue: "none" });
const typed: Context<string> = tenant;
const label: string | undefined = typed.name;

// The value type is inferred from `defaultValue` when it is not written out.
const inferred = createContext({ defaultValue: 42 });
const n: number = inferred.get();

// No options at all is legal; the value is then `unknown`.
const bare = createContext();
const unknownValue: unknown = bare.get();

// Options are optional individually, too.
const named = createContext<{ id: string }>({ name: "session" });
const options: ContextOptions<string> = { name: "x", defaultValue: "y" };

// @ts-expect-error — `defaultValue` must match the context's value type.
createContext<string>({ defaultValue: 1 });

// @ts-expect-error — `name` is a label, never a number.
createContext<string>({ name: 1 });

// --- get / run ----------------------------------------------------------------

const value: string = tenant.get();

// @ts-expect-error — `get()` takes nothing; there is no keyed lookup.
tenant.get("tenant");

// `run` returns exactly what the callback returned.
const returned: number = tenant.run("acme", () => 1);
const promised: Promise<string> = tenant.run("acme", async () => {
  await null;
  return tenant.get();
});
// ... and when the callback returns nothing, so does `run`. A `void` result
// cannot sit in a variable, so it is asserted through a `() => void` —
// which still fails to compile if the declaration ever returns something else.
const nothing: () => void = () => tenant.run("acme", () => {});
nothing();

// Trailing arguments are typed against the callback's parameters.
const summed: number = tenant.run("acme", (a: number, b: number) => a + b, 1, 2);

// @ts-expect-error — the trailing argument must match the parameter.
tenant.run("acme", (a: number) => a, "not a number");

// @ts-expect-error — too few arguments for the callback.
tenant.run("acme", (a: number, b: number) => a + b, 1);

// @ts-expect-error — the value must match the context's type.
tenant.run(1, () => 0);

// @ts-expect-error — `run` needs a function, not a value.
tenant.run("acme", "not a function");

// @ts-expect-error — the context object is read-only.
tenant.name = "other";

// --- snapshot -----------------------------------------------------------------

const resume = snapshot();
const fromSnapshot: number = resume(() => 1);
const fromSnapshotAsync: Promise<number> = resume(async () => 1);
const withArgs: string = resume((a: string, b: string) => a + b, "x", "y");

// @ts-expect-error — the trailing argument must match.
resume((a: number) => a, "x");

// @ts-expect-error — `snapshot()` itself takes no arguments.
snapshot(() => 1);

// --- bind ---------------------------------------------------------------------

const listener = (event: Event): void => void event.type;
const bound: (event: Event) => void = bind(listener);
new EventTarget().addEventListener("ping", bind(listener));

// The bound function keeps the signature it was given.
const adder = bind((a: number, b: number): number => a + b);
const added: number = adder(1, 2);

// @ts-expect-error — and therefore still rejects the wrong arguments.
adder("1", 2);

// @ts-expect-error — `bind` needs a function.
bind(42);

// --- withTrace ----------------------------------------------------------------

const traced: string = withTrace("0af7651916cd43dd8448eb211c80319c", () => tenant.get());
const tracedAsync: Promise<void> = withTrace("0af7651916cd43dd8448eb211c80319c", async () => {});

// @ts-expect-error — the trace id is a string, not bytes.
withTrace(new Uint8Array(16), () => 0);

// @ts-expect-error — `withTrace`'s callback takes no arguments (bind one in).
withTrace("0af7651916cd43dd8448eb211c80319c", (a: number) => a, 1);

// --- currentTask --------------------------------------------------------------

const task: TaskInfo = currentTask();
const id: number = task.id;
const parent: number | null = task.parentId;
const trace: string = task.traceId;
const kind: string = task.kind;

// @ts-expect-error — `parentId` is nullable at the root, so it is not a number.
const notNullable: number = task.parentId;

// --- what was deliberately removed -------------------------------------------

// @ts-expect-error — no `enterWith`: a scope with no end is the leak this avoids.
tenant.enterWith("acme");

// @ts-expect-error — no `exit`: `run(undefined, fn)` already expresses it.
tenant.exit(() => 0);

// @ts-expect-error — no `disable`: a global kill switch breaks every consumer.
tenant.disable();

// @ts-expect-error — `getStore` is Node's name; here it is `get`.
tenant.getStore();

export {
  added,
  adder,
  adder as _adder,
  bare,
  bound,
  fromSnapshot,
  fromSnapshotAsync,
  id,
  inferred,
  kind,
  label,
  n,
  named,
  nothing,
  notNullable,
  options,
  parent,
  promised,
  resume,
  returned,
  summed,
  task,
  trace,
  traced,
  tracedAsync,
  typed,
  unknownValue,
  value,
  withArgs,
};
