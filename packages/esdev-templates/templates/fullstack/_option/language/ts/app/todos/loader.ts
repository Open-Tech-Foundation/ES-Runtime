// Loaders run on the server and feed their page through `router.data`. A server
// function called from server code runs directly, with no HTTP request.
import { listTodos } from "./todos.server.ts";

export default async function loader({ locals }: { locals: Record<string, unknown> }) {
  return { todos: await listTodos(undefined, { locals }) };
}
