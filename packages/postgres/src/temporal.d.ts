/**
 * A local declaration of the parts of Temporal this driver uses.
 *
 * esrun ships Temporal, but TypeScript's libraries do not describe it yet and
 * `@opentf/esrun-types` does not either — so without this the driver cannot be
 * type-checked against a global the runtime definitely has. Deliberately narrow:
 * it covers what is used here and claims nothing about the rest, so that when
 * the types package grows a real declaration this file can be deleted rather
 * than reconciled.
 */
declare namespace Temporal {
  class Instant {
    static from(item: string): Instant;
    static fromEpochNanoseconds(nanoseconds: bigint): Instant;
    static fromEpochMilliseconds(milliseconds: number): Instant;
    readonly epochNanoseconds: bigint;
    readonly epochMilliseconds: number;
    toZonedDateTimeISO(timeZone: string): ZonedDateTime;
    toString(): string;
    toJSON(): string;
  }

  class ZonedDateTime {
    toPlainDateTime(): PlainDateTime;
    toInstant(): Instant;
    toString(): string;
  }

  class PlainDateTime {
    static from(item: string): PlainDateTime;
    add(duration: DurationLike): PlainDateTime;
    toString(): string;
    toJSON(): string;
  }

  class PlainDate {
    static from(item: string): PlainDate;
    add(duration: DurationLike): PlainDate;
    toString(): string;
    toJSON(): string;
  }

  class PlainTime {
    static from(item: string): PlainTime;
    add(duration: DurationLike): PlainTime;
    toString(): string;
    toJSON(): string;
  }

  class Duration {
    static from(item: string | DurationLike): Duration;
    toString(): string;
    toJSON(): string;
  }

  interface DurationLike {
    years?: number;
    months?: number;
    weeks?: number;
    days?: number;
    hours?: number;
    minutes?: number;
    seconds?: number;
    milliseconds?: number;
    microseconds?: number;
    nanoseconds?: number;
  }
}
