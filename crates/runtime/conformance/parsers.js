import { YAMLParser, YAMLBuilder, YAMLValidator, XMLParser, XMLBuilder, XMLValidator, TOMLParser, TOMLBuilder, TOMLValidator } from 'runtime:parsers';

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

assertEq(YAMLParser.parse(yamlData), expectedYamlParsed, "YAML parsing basic");

// YAML Validation Tests
assertEq(YAMLValidator.validate(yamlData), true, "YAML validation valid");
assertEq(YAMLValidator.validate(yamlData, { detailed: true }), { valid: true }, "YAML validation valid detailed");

const invalidYaml = `
name: Alice
  age: 30
`;

assertEq(YAMLValidator.validate(invalidYaml), false, "YAML validation invalid");
const invalidDetailed = YAMLValidator.validate(invalidYaml, { detailed: true });
if (invalidDetailed.valid !== false || typeof invalidDetailed.error !== 'string') {
    throw new Error("YAML validation invalid detailed failed");
}

assertThrows(() => YAMLParser.parse(invalidYaml), "YAML parse invalid throws");

// YAML Building Tests
const objToBuild = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtYaml = YAMLBuilder.build(objToBuild);
if (!builtYaml.includes("Bob") || !builtYaml.includes("42")) {
    throw new Error("YAML build failed: " + builtYaml);
}
assertEq(YAMLParser.parse(builtYaml), objToBuild, "YAML build back to obj");

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

assertEq(TOMLParser.parse(tomlData), expectedTomlParsed, "TOML parsing basic");

// TOML Validation Tests
assertEq(TOMLValidator.validate(tomlData), true, "TOML validation valid");
assertEq(TOMLValidator.validate(tomlData, { detailed: true }), { valid: true }, "TOML validation valid detailed");

const invalidToml = `
name = Alice
  age = 30
`;

assertEq(TOMLValidator.validate(invalidToml), false, "TOML validation invalid");
const tomlInvalidDetailed = TOMLValidator.validate(invalidToml, { detailed: true });
if (tomlInvalidDetailed.valid !== false || typeof tomlInvalidDetailed.error !== 'string') {
    throw new Error("TOML validation invalid detailed failed");
}

assertThrows(() => TOMLParser.parse(invalidToml), "TOML parse invalid throws");

// TOML Building Tests
const objToBuildToml = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtToml = TOMLBuilder.build(objToBuildToml);
if (!builtToml.includes("Bob") || !builtToml.includes("42")) {
    throw new Error("TOML build failed: " + builtToml);
}
assertEq(TOMLParser.parse(builtToml), objToBuildToml, "TOML build back to obj");

console.log("TOML tests passed!");

