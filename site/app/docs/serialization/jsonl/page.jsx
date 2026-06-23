import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export const metadata = {
  title: 'JSONL Processing | ES-Runtime Documentation',
  description: 'Native JSON Lines (JSONL) streaming in ES-Runtime',
}

export default function JSONLParserDoc() {
  return (
    <DocsShell active="/docs/serialization/jsonl">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        JSON Lines (JSONL)
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        In real-world systems, JSONL isn't just a file format—it's a persistent stream of records. That is the mental model most backend and data engineers use. It is less like "a file" and more like "a log that happens to be stored in a file."
      </p>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime provides pure streaming capabilities via <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">JSONL.DecoderStream</code> and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">JSONL.EncoderStream</code> to build robust data pipelines, analytics exports, and AI datasets.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Reading JSONL Streams
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">JSONL.DecoderStream</code> is a WHATWG TransformStream that safely decodes incoming text chunks, splitting them by newline and parsing each complete line into a JavaScript object.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { JSONL } from 'runtime:serialization';
import { file } from 'runtime:fs';

// Read a massive JSONL file natively from the file system
const decoder = new JSONL.DecoderStream({
  skipInvalid: true // Continue processing even if lines are corrupt
});

// Optionally log skipped/corrupted lines without crashing the pipeline
decoder.onError(err => {
  console.warn(\`Corrupt JSONL at line \${err.line}: \${err.raw}\`);
  console.error(err.cause);
});

const stream = file("users.jsonl")
  .stream()
  .pipeThrough(decoder);

for await (const user of stream) {
  // Process each parsed JavaScript object sequentially
  console.log("Loaded record:", user.id);
}`} title="reading_jsonl.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Writing JSONL Streams
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Writing is handled by piping JavaScript objects through the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">JSONL.EncoderStream</code>. This is typically used to append records continuously to a log.
      </p>
      
      <h3 className="mt-8 text-xl font-semibold text-zinc-900">
        Example 1: Application Logs
      </h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        This is probably the most common production use case. You can continuously append events to a log file as they occur.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { JSONL } from 'runtime:serialization';
import { file } from 'runtime:fs';

const log = new JSONL.EncoderStream();

log.pipeTo(
  file("access.log").writable({ append: true })
);

// Append logs as events happen
app.on("request", async req => {
  await log.write({
    timestamp: Date.now(),
    method: req.method,
    path: req.path,
    userId: req.userId,
  });
});`} title="logging.js" lang="js" />
      </div>

      <h3 className="mt-8 text-xl font-semibold text-zinc-900">
        Example 2: API → JSONL
      </h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Suppose you're collecting GitHub events to back up to a persistent log.
      </p>
      <div className="mt-6">
        <CodeBlock code={`const response = await fetch("https://api.github.com/users/octocat/events");
const events = await response.json();

for (const event of events) {
  await log.write({
    id: event.id,
    type: event.type,
    createdAt: event.created_at,
  });
}`} title="api_export.js" lang="js" />
      </div>

      <h3 className="mt-8 text-xl font-semibold text-zinc-900">
        Example 3: Database → JSONL
      </h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Very common for database exports, analytics backups, and AI training dataset generation.
      </p>
      <div className="mt-6">
        <CodeBlock code={`const users = await db.query(\`
  SELECT id, email, created_at
  FROM users
\`);

for (const user of users.rows) {
  await log.write(user);
}`} title="db_export.js" lang="js" />
      </div>

    </DocsShell>
  );
}
