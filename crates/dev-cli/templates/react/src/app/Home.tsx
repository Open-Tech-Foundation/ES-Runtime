import { useState } from "react";
import type { RouteData } from "./routes.ts";

export function Home({ data }: { data: RouteData }) {
  // State, so a full reload being a *full* reload is something you can feel:
  // count to three, save a file, and it is back to zero. That is the honest
  // cost of not having hot module replacement.
  const [count, setCount] = useState(0);

  return (
    <>
      <h1>{data.title}</h1>
      <p>{data.body}</p>
      <p>
        <button onClick={() => setCount(count + 1)}>counted {count}</button>
      </p>
      <p>
        Edit <code>src/app/Home.tsx</code> and this page reloads.
      </p>
    </>
  );
}
