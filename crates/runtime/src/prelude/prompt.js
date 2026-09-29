// prompt(), confirm() and alert() — the web's three modal questions, as Deno
// has them (DECISIONS D143).
//
// **Synchronous, like the web's.** A modal dialog stops the page until it is
// answered, and these stop the agent until a line arrives: no timer fires and
// no promise settles while one waits. That is what makes them usable at the
// top of a script without an `await`, and it is also why they are for
// command-line tools, not servers.
//
// **They do not wait on a pipe.** When standard input is not a terminal there
// is nobody to answer, and a question in a pipeline is a script that hangs. So
// `prompt` returns null, `confirm` returns false and `alert` returns at once —
// Deno's rule.
//
// **The question goes to standard error**, where Deno writes it to standard
// output: a program run as `tool > out.json` must still show its question, and
// must not write it into the file.
//
// The line is read from the same buffer `stdin` in runtime:process reads from,
// so an answer typed ahead of time is not lost between the two.
(() => {
  "use strict";
  // Made on first use: this fragment is baked into the startup snapshot, and
  // a decoder is host state that has no place in one.
  let encoder;
  let decoder;

  function ask(text) {
    encoder ??= new TextEncoder();
    decoder ??= new TextDecoder();
    __ops.process_write("stderr", encoder.encode(text));
    const bytes = __ops.process_stdin_read_line_sync();
    if (bytes === null) return null;
    let end = bytes.length;
    if (end > 0 && bytes[end - 1] === 0x0a) end--;
    if (end > 0 && bytes[end - 1] === 0x0d) end--;
    return decoder.decode(bytes.subarray(0, end));
  }

  const interactive = () => __ops.process_is_terminal("stdin");

  function alert(message = "Alert") {
    if (!interactive()) return;
    ask(`${message} [Enter] `);
  }

  function confirm(message = "Confirm") {
    if (!interactive()) return false;
    const answer = ask(`${message} [y/N] `);
    return answer === "y" || answer === "Y";
  }

  // An empty answer is the default, which is shown in brackets: there is no
  // line editor here to pre-fill it into, as Deno's is.
  function prompt(message = "Prompt", defaultValue) {
    const fallback = defaultValue === undefined || defaultValue === null ? null : `${defaultValue}`;
    if (!interactive()) return null;
    const shown = fallback ? `${message} [${fallback}] ` : `${message} `;
    const answer = ask(shown);
    if (answer === null) return null;
    return answer === "" && fallback !== null ? fallback : answer;
  }

  for (const [name, fn] of [
    ["alert", alert],
    ["confirm", confirm],
    ["prompt", prompt],
  ]) {
    Object.defineProperty(globalThis, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }
})();
