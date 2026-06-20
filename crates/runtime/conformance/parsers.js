test("parsers sync apis", async () => {
const { YAML, XML, TOML, MessagePack, JSONL } = await import('runtime:parsers');

function assertEq(actual, expected, msg) {
    function sortKeys(obj) {
        if (obj === null || typeof obj !== 'object') return obj;
        if (Array.isArray(obj)) return obj.map(sortKeys);
        return Object.keys(obj).sort().reduce((acc, key) => {
            acc[key] = sortKeys(obj[key]);
            return acc;
        }, {});
    }
    const actualStr = JSON.stringify(sortKeys(actual));
    const expectedStr = JSON.stringify(sortKeys(expected));
    if (actualStr !== expectedStr) {
        throw new Error(`${msg}: Expected ${expectedStr}, got ${actualStr}`);
    }
}

function assertThrows(fn, msg) {
    try {
        fn();
        throw new Error(`${msg}: Expected to throw, but did not`);
    } catch (e) {
        // Expected
    }
}

// YAML Parsing Tests
const yamlData = `
name: Alice
age: 30
is_active: true
roles:
  - admin
  - user
settings:
  theme: dark
`;

const expectedYamlParsed = {
    name: "Alice",
    age: 30,
    is_active: true,
    roles: ["admin", "user"],
    settings: {
        theme: "dark"
    }
};

assertEq(YAML.parse(yamlData), expectedYamlParsed, "YAML parsing basic");

// YAML Validation Tests
assertEq(YAML.validate(yamlData), true, "YAML validation valid");
assertEq(YAML.validate(yamlData, { detailed: true }), { valid: true }, "YAML validation valid detailed");

const invalidYaml = `
name: Alice
  age: 30
`;

assertEq(YAML.validate(invalidYaml), false, "YAML validation invalid");
const invalidDetailed = YAML.validate(invalidYaml, { detailed: true });
if (invalidDetailed.valid !== false || typeof invalidDetailed.error !== 'string') {
    throw new Error("YAML validation invalid detailed failed");
}

assertThrows(() => YAML.parse(invalidYaml), "YAML parse invalid throws");

// YAML Building Tests
const objToBuild = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtYaml = YAML.build(objToBuild);
if (!builtYaml.includes("Bob") || !builtYaml.includes("42")) {
    throw new Error("YAML build failed: " + builtYaml);
}
assertEq(YAML.parse(builtYaml), objToBuild, "YAML build back to obj");

console.log("YAML tests passed!");

// TOML Parsing Tests
const tomlData = `
name = "Alice"
age = 30
is_active = true
roles = ["admin", "user"]

[settings]
theme = "dark"
`;

const expectedTomlParsed = {
    name: "Alice",
    age: 30,
    is_active: true,
    roles: ["admin", "user"],
    settings: {
        theme: "dark"
    }
};

assertEq(TOML.parse(tomlData), expectedTomlParsed, "TOML parsing basic");

// TOML Validation Tests
assertEq(TOML.validate(tomlData), true, "TOML validation valid");
assertEq(TOML.validate(tomlData, { detailed: true }), { valid: true }, "TOML validation valid detailed");

const invalidToml = `
name = Alice
  age = 30
`;

assertEq(TOML.validate(invalidToml), false, "TOML validation invalid");
const tomlInvalidDetailed = TOML.validate(invalidToml, { detailed: true });
if (tomlInvalidDetailed.valid !== false || typeof tomlInvalidDetailed.error !== 'string') {
    throw new Error("TOML validation invalid detailed failed");
}

assertThrows(() => TOML.parse(invalidToml), "TOML parse invalid throws");

// TOML Building Tests
const objToBuildToml = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtToml = TOML.build(objToBuildToml);
if (!builtToml.includes("Bob") || !builtToml.includes("42")) {
    throw new Error("TOML build failed: " + builtToml);
}
assertEq(TOML.parse(builtToml), objToBuildToml, "TOML build back to obj");

console.log("TOML tests passed!");

// MessagePack Tests
const objToBuildMsgpack = {
    user: {
        name: "Charlie",
        id: 99
    }
};

const builtMsgpack = MessagePack.encode(objToBuildMsgpack);
if (!(builtMsgpack instanceof Uint8Array)) {
    throw new Error("MessagePack encode did not return Uint8Array");
}
assertEq(MessagePack.decode(builtMsgpack), objToBuildMsgpack, "MessagePack decode back to obj");
assertEq(MessagePack.validate(builtMsgpack), true, "MessagePack validation valid");

// Invalid msgpack
const invalidMsgpack = new Uint8Array([0xc1, 0x01]); // 0xc1 is never used
assertEq(MessagePack.validate(invalidMsgpack), false, "MessagePack validation invalid");
const msgpackInvalidDetailed = MessagePack.validate(invalidMsgpack, { detailed: true });
if (msgpackInvalidDetailed.valid !== false || typeof msgpackInvalidDetailed.error !== 'string') {
    throw new Error("MessagePack validation invalid detailed failed");
}
assertThrows(() => MessagePack.decode(invalidMsgpack), "MessagePack decode invalid throws");

console.log("MessagePack tests passed!");
});
