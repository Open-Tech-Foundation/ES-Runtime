// Server functions: this module runs only on the server. Components import the
// functions below and call them like async functions; the browser gets fetch
// stubs.
import { action, query } from "@opentf/afw/server";

// In memory for the example: use your database here.
const todos = ["Try the dev server", "Add an API route", "Deploy with npm start"];

export const listTodos = query(async () => todos);

export const addTodo = action(async ({ title }) => {
  const text = String(title ?? "").trim();
  if (!text) throw new Response(null, { status: 400 });
  todos.push(text);
  return todos;
});
