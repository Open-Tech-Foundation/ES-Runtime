// A first taste: console + standard globals.
console.log("Hello from ES-Runtime 👋");
console.log("self === globalThis:", self === globalThis);

const url = new URL("https://example.com/path?q=hi&n=2");
console.log("host:", url.host, "| q:", url.searchParams.get("q"));

const enc = new TextEncoder().encode("héllo😀");
console.log("UTF-8 byte length of 'héllo😀':", enc.length);

console.warn("this line goes to stderr");
