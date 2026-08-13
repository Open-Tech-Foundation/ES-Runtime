import type { RouteData } from "./routes.ts";

export function About({ data }: { data: RouteData }) {
  return (
    <>
      <h1>{data.title}</h1>
      <p>{data.body}</p>
      <p>
        This page is also written out as static HTML by <code>src/prerender.tsx</code>.
      </p>
    </>
  );
}
