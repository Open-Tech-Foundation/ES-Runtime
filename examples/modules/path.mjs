// runtime:path — modern, platform-aware path utilities. Run with:
//   esrun examples/modules/path.mjs
import { join, normalize, resolve, dirname, basename, extname, parse, relative, isAbsolute, sep, fromFileURL } from "runtime:path";

console.log("sep:", JSON.stringify(sep));
console.log("join:", join("a", "b", "..", "c/d/")); // a/c/d
console.log("normalize:", normalize("/a/./b/../c")); // /a/c
console.log("dirname:", dirname("/a/b/c.txt")); // /a/b
console.log("basename:", basename("/a/b/c.txt")); // c.txt
console.log("extname:", extname("archive.tar.gz")); // .gz
console.log("isAbsolute:", isAbsolute("/a"), isAbsolute("a")); // true false
console.log("relative:", relative("/a/b/c", "/a/x/y")); // ../../x/y
console.log("parse:", JSON.stringify(parse("/a/b/c.txt")));

// The modern __dirname: the directory of the current module.
const here = dirname(fromFileURL(import.meta.url));
console.log("here:", here);
console.log("resolve:", resolve("data", "x.json")); // <cwd>/data/x.json
