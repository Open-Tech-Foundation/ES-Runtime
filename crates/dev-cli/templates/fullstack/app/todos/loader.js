// Loaders run on the server and feed their page through `router.data`. A server
// function called from server code runs directly, with no HTTP request.
import { listTodos } from "./todos.server.js";

export default async function loader({ locals }) {
  return { todos: await listTodos(undefined, { locals }) };
}
