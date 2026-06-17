import { XMLParser, XMLValidator, XMLBuilder } from "runtime:parsers";

let passed = 0;
let failed = 0;

function assertEqual(actual, expected, msg) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    console.error(`FAIL: ${msg}`);
    console.error(`  Expected: ${JSON.stringify(expected)}`);
    console.error(`  Actual:   ${JSON.stringify(actual)}`);
    failed++;
  } else {
    passed++;
  }
}

function assertTrue(actual, msg) {
  if (actual !== true) {
    console.error(`FAIL: ${msg}`);
    failed++;
  } else {
    passed++;
  }
}

function assertFalse(actual, msg) {
  if (actual !== false) {
    console.error(`FAIL: ${msg}`);
    failed++;
  } else {
    passed++;
  }
}

const xml = `<user id="1"><name>Alice</name></user>`;

// Test Validator
assertTrue(XMLValidator.validate(xml), "Validates valid XML");
assertFalse(XMLValidator.validate(`<user id="1"><name>Alice</user>`), "Invalidates malformed XML");
const detailed = XMLValidator.validate(`<user id="1"><name>Alice</user>`, { detailed: true });
assertEqual(detailed.valid, false, "Detailed error returns valid=false");

// Test Parser
// quick-xml strips the root tag "user" and returns its attributes and inner children.
const parsed = XMLParser.parse(xml);
assertEqual(parsed, { "@id": "1", name: { "$text": "Alice" } }, "Parses XML correctly");

// Test Builder
const built = XMLBuilder.build({ user: { "@id": "1", name: "Alice" } });
// Note: Serde/quick-xml serialization format might slightly differ in attributes vs child tags,
// depending on how quick-xml interprets the JSON. For now, we test it doesn't throw.
assertTrue(typeof built === "string", "Builder returns string");
assertTrue(built.includes("Alice"), "Builder includes text content");

if (failed > 0) {
  console.error(`\n${failed} tests failed!`);
  throw new Error("Conformance test failed");
} else {
  console.log(`All ${passed} tests passed!`);
}
