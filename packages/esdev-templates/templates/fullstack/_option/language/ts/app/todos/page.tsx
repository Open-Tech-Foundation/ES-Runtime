import { invalidate, router } from "@opentf/afw";
import { addTodo } from "./todos.server.ts";

export const metadata = { title: "Todos" };

export default function Todos() {
  let title = $state("");
  const todos = $derived((router.data as { todos?: string[] } | undefined)?.todos ?? []);

  async function add(event: SubmitEvent) {
    event.preventDefault();
    await addTodo({ title });
    title = "";
    await invalidate(); // reload this page's loader data
  }

  return (
    <section>
      <h1>Todos</h1>
      <ul>
        {todos.map((todo: string) => <li>{todo}</li>)}
      </ul>
      <form onsubmit={add}>
        <input value={title} oninput={(e: Event) => (title = (e.target as HTMLInputElement).value)} placeholder="New todo" />
        <button type="submit">Add</button>
      </form>
    </section>
  );
}
