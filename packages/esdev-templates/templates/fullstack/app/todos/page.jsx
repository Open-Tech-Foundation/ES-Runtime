import { invalidate, router } from "@opentf/afw";
import { addTodo } from "./todos.server.js";

export const metadata = { title: "Todos" };

export default function Todos() {
  let title = $state("");
  const todos = $derived(router.data?.todos ?? []);

  async function add(event) {
    event.preventDefault();
    await addTodo({ title });
    title = "";
    await invalidate(); // reload this page's loader data
  }

  return (
    <section>
      <h1>Todos</h1>
      <ul>
        {todos.map((todo) => <li>{todo}</li>)}
      </ul>
      <form onsubmit={add}>
        <input value={title} oninput={(e) => (title = e.target.value)} placeholder="New todo" />
        <button type="submit">Add</button>
      </form>
    </section>
  );
}
