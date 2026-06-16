import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const BASIC = `const pattern = new URLPattern('/api/users/:id', 'https://api.example.com');
console.log(pattern.test('https://api.example.com/api/users/123')); // true
console.log(pattern.test('https://api.example.com/api/posts/123')); // false

const result = pattern.exec('https://api.example.com/api/users/456');
console.log(result.pathname.groups.id); // "456"`;

const OBJECT_FORM = `const pattern = new URLPattern({
  protocol: 'http*',
  hostname: '*.example.com',
  pathname: '/data/:type/*'
});

const result = pattern.exec('https://api.example.com/data/images/avatar.png');
console.log(result.pathname.groups.type); // "images"
console.log(result.pathname.groups['0']); // "avatar.png" (from wildcard *)`;

export default function URLPatternDoc() {
  return (
    <DocsShell active="/docs/urlpattern">
      <p className="text-sm font-medium text-brand-600">Web APIs</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        URLPattern
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun provides a hyper-optimized, native-JavaScript implementation of the{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">URLPattern</code> Web API. It allows you to match URLs
        and extract data (like route parameters and wildcards) cleanly and efficiently.
      </p>

      <div className="mt-6 rounded-xl border border-zinc-200 bg-zinc-50 p-5 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Performance edge:</strong>{" "}
        Our implementation caches native JavaScript <code className="rounded bg-white px-1.5 py-0.5 text-[12px]">RegExp</code>{" "}
        instances per component, allowing 50,000 instantiations to execute in <strong>~187ms</strong>, drastically
        outperforming both Node.js (C++) and Bun (Zig).
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Basic Usage</h2>
      <p className="mt-3 text-zinc-600">
        You can construct a pattern using a string and an optional base URL. Use{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">test()</code> to check if a URL matches, and{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">exec()</code> to extract parameters.
      </p>
      <div className="mt-4">
        <CodeBlock code={BASIC} title="app.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Object Patterns</h2>
      <p className="mt-3 text-zinc-600">
        For more complex matching, you can pass an object defining specific patterns for each URL component
        (protocol, hostname, pathname, etc).
      </p>
      <div className="mt-4">
        <CodeBlock code={OBJECT_FORM} title="app.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Syntax features</h2>
      <ul className="mt-4 list-disc space-y-2 pl-5 text-zinc-600">
        <li>
          <strong>Named groups:</strong> Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">:param</code> to extract a
          segment into <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">groups.param</code>.
        </li>
        <li>
          <strong>Wildcards:</strong> Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">*</code> to match
          everything up to the end of the component. Wildcards are indexed numerically in the groups object.
        </li>
        <li>
          <strong>Ignore Case:</strong> Pass <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">{`{ ignoreCase: true }`}</code>{" "}
          to the constructor options to match case-insensitively.
        </li>
      </ul>
    </DocsShell>
  );
}
