import DocsShell from "../../../components/DocsShell.jsx";
import StatusIcon from "../../../components/StatusIcon.jsx";

// Web-standard globals available in esrun (sourced from the runtime prelude).
const groups = [
  {
    title: "Core",
    items: [
      { n: "globalThis", s: "yes" },
      { n: "self", s: "yes" },
      { n: "console", s: "yes" },
      { n: "queueMicrotask", s: "yes" },
      { n: "structuredClone", s: "yes" },
      { n: "reportError", s: "yes" },
    ],
  },
  {
    title: "Timers",
    items: [
      { n: "setTimeout", s: "yes" },
      { n: "clearTimeout", s: "yes" },
      { n: "setInterval", s: "yes" },
      { n: "clearInterval", s: "yes" },
    ],
  },
  {
    title: "URL",
    items: [
      { n: "URL", s: "yes" },
      { n: "URLSearchParams", s: "yes" },
    ],
  },
  {
    title: "Fetch & networking",
    items: [
      { n: "fetch", s: "yes" },
      { n: "Request", s: "yes" },
      { n: "Response", s: "yes" },
      { n: "Headers", s: "yes" },
    ],
  },
  {
    title: "Encoding",
    items: [
      { n: "TextEncoder", s: "yes" },
      { n: "TextDecoder", s: "yes" },
      { n: "TextEncoderStream", s: "yes" },
      { n: "TextDecoderStream", s: "yes" },
      { n: "atob", s: "yes" },
      { n: "btoa", s: "yes" },
    ],
  },
  {
    title: "Streams",
    items: [
      { n: "ReadableStream", s: "yes" },
      { n: "WritableStream", s: "yes" },
      { n: "TransformStream", s: "yes" },
      { n: "ByteLengthQueuingStrategy", s: "yes" },
      { n: "CountQueuingStrategy", s: "yes" },
    ],
  },
  {
    title: "Crypto",
    items: [
      { n: "crypto", s: "yes", note: "getRandomValues, randomUUID" },
      { n: "crypto.subtle", s: "yes", note: "digest, HMAC, AES-GCM/CBC/CTR, HKDF, PBKDF2" },
      { n: "CryptoKey", s: "yes" },
    ],
  },
  {
    title: "Events",
    items: [
      { n: "Event", s: "yes" },
      { n: "EventTarget", s: "yes" },
      { n: "CustomEvent", s: "yes" },
      { n: "AbortController", s: "yes" },
      { n: "AbortSignal", s: "yes" },
    ],
  },
  {
    title: "Data",
    items: [
      { n: "Blob", s: "yes" },
      { n: "File", s: "yes" },
      { n: "FormData", s: "yes" },
      { n: "DOMException", s: "yes" },
    ],
  },
  {
    title: "Performance",
    items: [{ n: "performance", s: "yes", note: "now(), timeOrigin" }],
  },
];

const notAvailable = [
  { n: "process / Buffer / require", why: "Node.js globals — not provided (use runtime: modules)" },
  { n: "Worker / MessageChannel", why: "no Workers in Layer A (see Scope)" },
  { n: "WebSocket", why: "not yet implemented" },
  { n: "navigator / localStorage / window", why: "browser globals — out of scope" },
];

export default function GlobalsDoc() {
  return (
    <DocsShell active="/docs/globals">
      <p className="text-sm font-medium text-brand-600">Web standard APIs</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Global objects
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun's global scope tracks the WinterTC Minimum Common Web Platform
        API. These are standard Web globals — the same names you would use in a
        browser or other server runtimes. Host capabilities (filesystem,
        process, network access) are <em>not</em> globals; they live in{" "}
        <a href="/docs/modules" className="font-medium text-brand-600 hover:text-brand-700">
          runtime: modules
        </a>
        .
      </p>

      {groups.map((g) => (
        <div className="mt-10">
          <h2 className="text-sm font-semibold uppercase tracking-wider text-zinc-500">
            {g.title}
          </h2>
          <div className="mt-3 overflow-hidden rounded-xl border border-zinc-200">
            <table className="w-full text-left text-sm">
              <tbody className="divide-y divide-zinc-100">
                {g.items.map((it) => (
                  <tr>
                    <td className="w-8 py-2.5 pl-4">
                      <StatusIcon status={it.s} />
                    </td>
                    <td className="py-2.5 pr-4 font-mono text-[13px] text-zinc-800">
                      <span className="ml-3">{it.n}</span>
                    </td>
                    <td className="py-2.5 pr-4 text-zinc-500">
                      {it.note || ""}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ))}

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Not available
      </h2>
      <div className="mt-3 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <tbody className="divide-y divide-zinc-100">
            {notAvailable.map((it) => (
              <tr>
                <td className="w-8 py-2.5 pl-4">
                  <StatusIcon status="no" />
                </td>
                <td className="py-2.5 pr-4 font-mono text-[13px] text-zinc-700">
                  <span className="ml-3">{it.n}</span>
                </td>
                <td className="py-2.5 pr-4 text-zinc-500">{it.why}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </DocsShell>
  );
}
