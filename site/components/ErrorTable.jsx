// A 2-column Error / Thrown-when table for the API reference pages. One shared
// definition (each component name registers one custom element, so this must not
// be duplicated per page). `rows` is [{ e: errorType, w: whenThrown }].
export default function ErrorTable({ rows }) {
  return (
    <div className="mt-4 overflow-hidden rounded-xl border border-zinc-200">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
          <tr>
            <th className="px-4 py-3 font-semibold">Error</th>
            <th className="px-4 py-3 font-semibold">Thrown when</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100">
          {rows.map((x) => (
            <tr>
              <td className="whitespace-nowrap px-4 py-3 font-mono text-[13px] font-medium text-zinc-900">{x.e}</td>
              <td className="px-4 py-3 text-zinc-600">{x.w}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
