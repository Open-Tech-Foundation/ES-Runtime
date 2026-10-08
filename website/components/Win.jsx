// Winner highlight for hand-written benchmark tables: the same semibold green
// ImagesTable gives its computed best cells, for markdown tables whose winners
// are marked by hand. Used by app/docs/benchmarks/page.mdx.
export default function Win({ children }) {
  return (
    <span className="font-semibold text-emerald-600 dark:text-emerald-400">{children}</span>
  );
}
