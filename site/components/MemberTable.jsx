// A 3-column Member / Type / Description table, shared by the API reference
// pages (a single definition: each component name registers one custom element,
// so this must not be duplicated per page).
export default function MemberTable({ rows }) {
  return (
    <div className="mt-4 overflow-hidden rounded-xl border border-zinc-200">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
          <tr>
            <th className="px-4 py-3 font-semibold">Member</th>
            <th className="px-4 py-3 font-semibold">Type</th>
            <th className="px-4 py-3 font-semibold">Description</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100">
          {rows.map((x) => (
            <tr>
              <td className="px-4 py-3 font-mono text-[13px] font-medium text-zinc-900">{x.m}</td>
              <td className="px-4 py-3 font-mono text-[13px] text-zinc-500">{x.t}</td>
              <td className="px-4 py-3 text-zinc-600">{x.d}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
