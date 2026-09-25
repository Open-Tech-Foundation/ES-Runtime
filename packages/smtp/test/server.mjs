// A scriptable SMTP server over runtime:net, for the protocol tests.
//
// Not a mail server: it accepts what it is told to, refuses what it is told
// to, and records every command and message, so a test can assert what went
// over the wire. The real-server tests (mailpit.mjs) check the same client
// against an implementation that is not ours.
import { listen } from "runtime:net";

const defaults = {
  extensions: [
    "PIPELINING",
    "SIZE 1048576",
    "8BITMIME",
    "SMTPUTF8",
    "AUTH PLAIN LOGIN XOAUTH2",
    "ENHANCEDSTATUSCODES",
  ],
  ehlo: 250,
  mail: 250,
  rcpt: () => 250,
  data: 354,
  final: 250,
  user: "user",
  password: "secret",
  token: "token",
  // Close the connection after this many messages, as a server enforcing a
  // per-session limit or an idle disconnect would.
  dropAfter: Infinity,
};

export async function startServer(options = {}) {
  const behaviour = { ...defaults, ...options };
  const server = listen({ hostname: "127.0.0.1", port: 0 });
  const { port } = await server.addr;
  const state = { port, commands: [], messages: [], connections: 0 };

  (async () => {
    for await (const socket of server) {
      state.connections++;
      session(socket, behaviour, state).catch(() => {});
    }
  })().catch(() => {});

  return { ...state, state, close: () => server.close() };
}

async function session(socket, b, state) {
  const writer = socket.writable.getWriter();
  const encoder = new TextEncoder();
  const say = (line) => writer.write(encoder.encode(`${line}\r\n`));
  const lines = readLines(socket.readable);
  let authed = false;
  let sent = 0;
  let envelope = null;

  await say("220 test.local ESMTP ready");
  for (;;) {
    const { value: line, done } = await lines.next();
    if (done) return;
    state.commands.push(line);
    const verb = line.split(" ")[0].toUpperCase();
    if (verb === "EHLO") {
      if (b.ehlo !== 250) {
        await say(`${b.ehlo} 5.5.1 EHLO not understood`);
        continue;
      }
      const ext = ["test.local", ...b.extensions];
      for (const [i, e] of ext.entries()) await say(`250${i === ext.length - 1 ? " " : "-"}${e}`);
    } else if (verb === "HELO") {
      await say("250 test.local");
    } else if (verb === "AUTH") {
      authed = await auth(line, b, say, lines, state);
    } else if (verb === "MAIL") {
      envelope = {
        from: /<([^>]*)>/.exec(line)?.[1],
        to: [],
        params: line.split(">")[1]?.trim() ?? "",
      };
      await say(b.mail === 250 ? "250 2.1.0 sender ok" : `${b.mail} 5.1.8 sender refused`);
      if (b.mail !== 250) envelope = null;
    } else if (verb === "RCPT") {
      const address = /<([^>]*)>/.exec(line)?.[1];
      const code = b.rcpt(address);
      if (code === 250 && envelope) envelope.to.push(address);
      await say(
        code === 250 ? "250 2.1.5 recipient ok" : `${code} 5.1.1 <${address}>: user unknown`,
      );
    } else if (verb === "DATA") {
      if (!envelope || envelope.to.length === 0) {
        await say("554 5.5.1 no valid recipients");
        continue;
      }
      if (b.data !== 354) {
        await say(`${b.data} 5.3.4 not now`);
        continue;
      }
      await say("354 end with <CRLF>.<CRLF>");
      const body = [];
      for (;;) {
        const { value, done: ended } = await lines.next();
        if (ended) return;
        if (value === ".") break;
        body.push(value.startsWith(".") ? value.slice(1) : value);
      }
      state.messages.push({ ...envelope, data: body.join("\r\n"), authed });
      envelope = null;
      await say(
        b.final === 250
          ? `250 2.0.0 queued as Q${state.messages.length}`
          : `${b.final} 5.7.1 content refused`,
      );
      if (++sent >= b.dropAfter) {
        await socket.close();
        return;
      }
    } else if (verb === "RSET") {
      envelope = null;
      await say("250 2.0.0 reset");
    } else if (verb === "NOOP") {
      await say("250 2.0.0 ok");
    } else if (verb === "QUIT") {
      await say("221 2.0.0 bye");
      await socket.close();
      return;
    } else {
      await say("502 5.5.2 command not implemented");
    }
  }
}

async function auth(line, b, say, lines, state) {
  const [, mechanism, initial] = line.split(" ");
  const decode = (s) => new TextDecoder().decode(Uint8Array.from(atob(s), (c) => c.charCodeAt(0)));
  let ok = false;
  if (mechanism === "PLAIN") {
    const [, user, password] = decode(initial).split("\0");
    ok = user === b.user && password === b.password;
  } else if (mechanism === "LOGIN") {
    await say("334 VXNlcm5hbWU6");
    const user = decode((await lines.next()).value);
    await say("334 UGFzc3dvcmQ6");
    const password = decode((await lines.next()).value);
    ok = user === b.user && password === b.password;
  } else if (mechanism === "XOAUTH2") {
    const token = decode(initial)
      .split("\u0001")
      .find((field) => field.startsWith("auth=Bearer "))
      ?.slice("auth=Bearer ".length);
    ok = token === b.token;
    if (!ok) {
      await say(`334 ${btoa('{"status":"401"}')}`);
      await lines.next();
    }
  }
  state.auth = mechanism;
  await say(ok ? "235 2.7.0 authenticated" : "535 5.7.8 authentication failed");
  return ok;
}

async function* readLines(readable) {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of readable) {
    buffer += decoder.decode(chunk, { stream: true });
    let end;
    while ((end = buffer.indexOf("\r\n")) !== -1) {
      yield buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
    }
  }
}
