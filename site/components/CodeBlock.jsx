// A dark code panel with lightweight, dependency-free syntax highlighting.
// Pass `code` as a string, an optional `title`, and `lang` ("js"/"ts"/"sh"/…).
// A tiny tokenizer colors comments, strings, keywords, and numbers — no
// highlighting library, so it stays crisp and fast.

const COMMENT = "text-zinc-500 italic";
const STRING = "text-emerald-400";
const KEYWORD = "text-sky-400";
const NUMBER = "text-amber-300";

// JS/TS: comments, template/double/single strings, keywords, numbers.
const JS_RE =
  /\/\/[^\n]*|\/\*[\s\S]*?\*\/|`(?:\\[\s\S]|[^`\\])*`|"(?:\\.|[^"\\\n])*"|'(?:\\.|[^'\\\n])*'|\b(?:const|let|var|function|return|if|else|for|while|do|await|async|import|export|from|as|new|class|extends|super|this|try|catch|finally|throw|typeof|instanceof|of|in|default|delete|void|yield|static|get|set|null|true|false|undefined)\b|\b\d[\w.]*\b/g;
// Shell: comments and strings.
const SH_RE = /#[^\n]*|"(?:\\.|[^"\\])*"|'[^']*'/g;

function classOf(token) {
  if (token.startsWith("//") || token.startsWith("/*") || token[0] === "#") return COMMENT;
  const q = token[0];
  if (q === '"' || q === "'" || q === "`") return STRING;
  if (token[0] >= "0" && token[0] <= "9") return NUMBER;
  return KEYWORD;
}

// Splits `code` into styled/plain tokens for the language (uses a while loop,
// never `.map`, so the compiler doesn't rewrite it as a reactive list).
function tokenize(code, lang) {
  const re = lang === "sh" || lang === "bash" || lang === "text" ? (lang === "text" ? null : SH_RE) : JS_RE;
  if (!re) return [{ t: code, c: "" }];
  const out = [];
  let last = 0;
  let m;
  re.lastIndex = 0;
  while ((m = re.exec(code)) !== null) {
    if (m.index > last) out.push({ t: code.slice(last, m.index), c: "" });
    out.push({ t: m[0], c: classOf(m[0]) });
    last = re.lastIndex;
    if (re.lastIndex === m.index) re.lastIndex++; // guard against zero-width
  }
  if (last < code.length) out.push({ t: code.slice(last), c: "" });
  return out;
}

export default function CodeBlock({ code, title, lang }) {
  const tokens = tokenize(code, lang);
  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950 shadow-sm">
      {title && (
        <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-2.5">
          <span className="text-xs font-medium text-zinc-400">{title}</span>
          {lang && (
            <span className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
              {lang}
            </span>
          )}
        </div>
      )}
      <pre className="overflow-x-auto px-4 py-4 text-[13px] leading-relaxed text-zinc-100">
        <code>
          {tokens.map((tok) => (
            <span className={tok.c}>{tok.t}</span>
          ))}
        </code>
      </pre>
    </div>
  );
}
