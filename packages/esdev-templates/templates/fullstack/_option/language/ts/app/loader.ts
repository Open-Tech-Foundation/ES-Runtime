// Runs on the server; the page reads what it returns as `router.data`.
export default function (_context: {
  params: Record<string, string>;
  query: Record<string, string>;
}) {
  return { message: "Hello, world!" };
}
