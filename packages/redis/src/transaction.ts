/**
 * `MULTI`/`EXEC`, which is not what `transaction(fn)` means.
 *
 * Redis queues the commands and applies them together with nothing interleaved,
 * and that is genuinely useful. What it does **not** do is roll back: a command
 * that fails at `EXEC` time leaves the ones beside it applied. So this is not
 * wired to `runtime:db`'s `transaction()` — which promises that a body that
 * throws changes nothing — and `supports.transactions` stays `false`. Naming it
 * `multi()` after the command it sends is the honest spelling.
 *
 * Commands are buffered rather than sent as they are written, which is why the
 * whole transaction costs one round trip and why a **pool** can run one: there
 * is nothing to hold a connection for until `exec()`.
 */
import { DbError, DbErrorCode } from "runtime:db";

import { RedisCommands, registerTransaction, type TransactionRunner } from "./commands.js";
import type { CommandArg } from "./protocol/resp.js";

export type { TransactionRunner };

export class RedisTransaction extends RedisCommands {
  readonly #runner: TransactionRunner;
  readonly #commands: CommandArg[][] = [];
  readonly #settlers: ((value: unknown) => void)[] = [];
  #executed = false;

  constructor(runner: TransactionRunner) {
    super();
    this.#runner = runner;
  }

  /**
   * Refused: Redis has no nested transaction, and `MULTI` inside `MULTI` is an
   * error the server would reject at queue time anyway.
   */
  override execTransaction(): Promise<unknown[] | null> {
    return Promise.reject(
      new DbError("transactions do not nest — MULTI inside MULTI is not a Redis thing", {
        code: DbErrorCode.Unsupported,
      }),
    );
  }

  /** How many commands are queued. */
  get size(): number {
    return this.#commands.length;
  }

  /**
   * Queues a command instead of running it.
   *
   * Every helper on {@link RedisCommands} routes through here, so `tx.set(…)`
   * and `tx.incr(…)` queue exactly as `tx.call(["SET", …])` does — which is why
   * a transaction has the whole command surface without reimplementing it.
   *
   * The returned promise resolves when {@link exec} runs, with that command's
   * own result — which is a {@link DbError} if that command failed, exactly as
   * the entry in `exec()`'s array is. **Do not await it before `exec()`**:
   * nothing will have settled it yet, and the await would wait forever.
   *
   * It **resolves** rather than rejects, and that is deliberate rather than
   * lazy. Every helper on `RedisCommands` is `async` and returns a promise of
   * its own derived from this one, so a rejection here rejects a promise the
   * caller usually never took — `tx.set("a", 1)` is written for its effect, not
   * its value. Rejecting would make the ordinary use of a transaction whose
   * commands failed produce one unhandled rejection per queued command, all
   * pointing at lines that did nothing wrong. Errors are values here for the
   * same reason they are values in `exec()`'s array: nothing was undone, so
   * there is a result either way.
   */
  override call(args: readonly CommandArg[]): Promise<unknown> {
    if (this.#executed) {
      throw new DbError(
        "this transaction has already been executed — build a new one with multi()",
        { code: DbErrorCode.Unsupported },
      );
    }
    this.#commands.push([...args]);
    return new Promise<unknown>((resolve) => {
      this.#settlers.push(resolve);
    });
  }

  /**
   * Sends the transaction and returns one result per queued command.
   *
   * `null` means `EXEC` was aborted because a `WATCH`ed key changed — the
   * optimistic-locking outcome, not an error. Read again and retry.
   *
   * A result may itself be a `DbError`, because a command that fails at
   * execution time does not roll back the ones beside it. Throwing would
   * discard the results of the commands that *did* apply, so the errors are
   * handed over in place and it is the caller's to decide:
   *
   * ```js
   * for (const result of await tx.exec() ?? []) {
   *   if (result instanceof DbError) report(result);
   * }
   * ```
   */
  async exec(): Promise<unknown[] | null> {
    if (this.#executed) {
      throw new DbError("this transaction has already been executed", {
        code: DbErrorCode.Unsupported,
      });
    }
    this.#executed = true;
    if (this.#commands.length === 0) return [];

    let results: unknown[] | null;
    try {
      results = await this.#runner.execTransaction(this.#commands);
    } catch (e) {
      // Thrown to the caller of `exec()`, who asked; handed to the queued
      // promises as a value, since nobody necessarily took those.
      for (const settle of this.#settlers) settle(e);
      throw e;
    }

    if (results === null) {
      // Nothing ran. A watched key moved under us, which is exactly what
      // `ERR_DB_SERIALIZATION_FAILURE` means everywhere else in `runtime:db` —
      // so the per-command promises say that rather than inventing a name.
      const aborted = new DbError(
        "the transaction was not applied: a WATCHed key changed before EXEC",
        { code: DbErrorCode.SerializationFailure, backendCode: "EXECABORT" },
      );
      for (const settle of this.#settlers) settle(aborted);
      return null;
    }

    for (let i = 0; i < this.#settlers.length; i++) this.#settlers[i]!(results[i]);
    return results;
  }

  /** Throws the queued commands away without sending anything. */
  discard(): void {
    this.#executed = true;
    const discarded = new DbError("the transaction was discarded", {
      code: DbErrorCode.Unsupported,
    });
    for (const settle of this.#settlers) settle(discarded);
    this.#commands.length = 0;
  }
}

// Handed to `commands.ts` rather than imported by it: the two modules need each
// other, and this is the direction that can be a run-time edge instead of a
// load-time cycle.
registerTransaction(RedisTransaction);
