// Topic names, filters, and matching. These are the rules a broker would
// enforce by refusing the subscription, caught here where the message can still
// be useful; and the two subtleties — `#` at the end only, and a wildcard never
// reaching the `$`-reserved space — are exactly the ones worth pinning.
import { exit } from "runtime:process";
import {
  parseSharedFilter,
  topicMatches,
  validateTopicFilter,
  validateTopicName,
} from "../../dist/protocol/topic.js";
import { ok, report, throws } from "./assert.mjs";

function accepts(fn, value, what) {
  try {
    fn(value);
    ok(true, what);
  } catch {
    ok(false, what);
  }
}

// Topic names: concrete, no wildcards.
accepts(validateTopicName, "sport/tennis/player1", "a plain topic name is valid");
accepts(validateTopicName, "sport/tennis/player1/ranking", "a deeper topic name is valid");
accepts(validateTopicName, "$SYS/broker/uptime", "a $SYS topic name is valid");
accepts(validateTopicName, "/", "a lone slash is a valid (if odd) topic name");
await throws(() => validateTopicName("sport/+"), "a plus in a topic name is refused");
await throws(() => validateTopicName("sport/#"), "a hash in a topic name is refused");
await throws(() => validateTopicName(""), "an empty topic name is refused");

// Topic filters: wildcards allowed, in the positions the spec fixes.
accepts(validateTopicFilter, "sport/tennis/player1", "a plain filter is valid");
accepts(validateTopicFilter, "#", "a lone hash is valid");
accepts(validateTopicFilter, "sport/#", "a trailing hash is valid");
accepts(validateTopicFilter, "sport/+/player1", "a plus level is valid");
accepts(validateTopicFilter, "+", "a lone plus is valid");
accepts(validateTopicFilter, "+/+", "two plus levels are valid");
accepts(validateTopicFilter, "$SYS/#", "$SYS filters are valid");
accepts(validateTopicFilter, "$share/group/sport/+", "a shared filter is valid");
await throws(() => validateTopicFilter("sport/#/ranking"), "a hash not at the end is refused");
await throws(() => validateTopicFilter("sport#"), "a hash inside a level is refused");
await throws(() => validateTopicFilter("sport+/player"), "a plus inside a level is refused");
await throws(() => validateTopicFilter(""), "an empty filter is refused");
await throws(() => validateTopicFilter("$share//sport"), "an empty share group is refused");
await throws(
  () => validateTopicFilter("$share/group"),
  "a shared filter with no filter is refused",
);

// Shared-filter splitting.
const shared = parseSharedFilter("$share/workers/jobs/+");
ok(
  shared !== null && shared.group === "workers" && shared.filter === "jobs/+",
  "a shared filter splits",
);
ok(parseSharedFilter("jobs/+") === null, "an ordinary filter is not shared");

// Matching, the rule a broker would apply.
ok(topicMatches("sport/tennis/player1", "sport/tennis/player1"), "an exact filter matches");
ok(topicMatches("sport/+/player1", "sport/tennis/player1"), "+ matches one level");
ok(topicMatches("sport/#", "sport/tennis/player1/ranking"), "# matches many levels");
ok(topicMatches("sport/#", "sport"), "# matches the parent level too");
ok(topicMatches("#", "any/thing/at/all"), "# alone matches everything");
ok(topicMatches("+/+", "a/b"), "+/+ matches two levels");
ok(topicMatches("$share/group/sport/#", "sport/tennis"), "a shared filter matches on its filter");
ok(topicMatches("$SYS/#", "$SYS/broker/uptime"), "$SYS/# reaches the reserved space");
ok(topicMatches("sport/+/player1", "sport/tennis") === false, "+ does not match a missing level");
ok(
  topicMatches("sport/tennis", "sport/tennis/player1") === false,
  "a shorter filter does not match",
);
ok(topicMatches("#", "$SYS/broker") === false, "a leading wildcard never reaches $-topics");
ok(topicMatches("+/broker", "$SYS/broker") === false, "a leading + never reaches $-topics");

exit(report("topic") > 0 ? 1 : 0);
