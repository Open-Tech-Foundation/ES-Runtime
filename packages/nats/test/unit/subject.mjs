// Subjects: what is a valid subject, what is a valid filter, and what matches.
//
// The broker routes, so the client's own matcher is not on the delivery path —
// but the rules are the ones the tests and a stub server reason about, and the
// `$`-reserved case and the `>`-needs-a-token case are easy to get wrong.
import { exit } from "runtime:process";
import { createInbox, subjectMatches, validateQueue, validateSubject } from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";

// -- publishing subjects (no wildcards) -------------------------------------

validateSubject("a", false);
validateSubject("a.b.c", false);
validateSubject("_INBOX.abc.123", false);
validateSubject("$SYS.REQ.x", false);
ok(true, "plain subjects are valid");

await throws(() => validateSubject("", false), "an empty subject is refused");
await throws(() => validateSubject("a..b", false), "an empty token is refused");
await throws(() => validateSubject(".a", false), "a leading dot is refused");
await throws(() => validateSubject("a.", false), "a trailing dot is refused");
await throws(() => validateSubject("a b", false), "whitespace is refused");
await throws(() => validateSubject("a\tb", false), "a tab is refused");
await throws(() => validateSubject("a.*", false), "a wildcard is refused when publishing");
await throws(() => validateSubject("a.>", false), "a `>` is refused when publishing");
await throws(() => validateSubject("a*", false), "a partial wildcard is refused");

// -- subscription filters (wildcards allowed) -------------------------------

validateSubject("*", true);
validateSubject("a.*", true);
validateSubject("a.>", true);
validateSubject(">", true);
validateSubject("a.*.b", true);
ok(true, "wildcards are valid in a filter");

await throws(() => validateSubject("a.>.b", true), "`>` must be the last token");
await throws(() => validateSubject("a*", true), "a wildcard must own its whole token");
validateSubject("a.x#", true);
ok(true, "`#` is an ordinary character — NATS uses `>` for the tail");

// -- queue groups -----------------------------------------------------------

validateQueue("workers");
ok(true, "a queue group name is valid");
await throws(() => validateQueue(""), "an empty queue group is refused");
await throws(() => validateQueue("two words"), "a queue group with a space is refused");

// -- matching ---------------------------------------------------------------

ok(subjectMatches("a.*", "a.b"), "`*` matches one token");
ok(!subjectMatches("a.*", "a.b.c"), "`*` does not match two tokens");
ok(!subjectMatches("a.*", "a"), "`*` needs a token to match");
ok(subjectMatches("a.>", "a.b.c"), "`>` matches one or more tokens");
ok(subjectMatches("a.>", "a.b"), "`>` matches a single trailing token");
ok(!subjectMatches("a.>", "a"), "`>` needs at least one token after");
ok(subjectMatches(">", "anything.at.all"), "a bare `>` matches everything");
ok(subjectMatches("a.b", "a.b"), "a literal filter matches exactly");
ok(!subjectMatches("a.b", "a.c"), "a literal filter does not match another token");

// The `$`-prefixed space is the server's; a filter must begin there to reach it.
ok(subjectMatches("$SYS.>", "$SYS.x"), "a `$`-prefixed filter reaches the reserved space");
ok(!subjectMatches("*", "$SYS.x"), "a leading wildcard never reaches the reserved space");
ok(!subjectMatches(">", "$SYS.x"), "a bare `>` never reaches the reserved space");

// -- inbox ------------------------------------------------------------------

const inbox = createInbox();
ok(inbox.startsWith("_INBOX."), "an inbox carries the conventional prefix");
is(inbox.length, "_INBOX.".length + 22, "and a 22-character token");
const first = createInbox();
const second = createInbox();
ok(first !== second, "two inboxes differ");

exit(report("subject") > 0 ? 1 : 0);
