import { Link } from "react-router";

export function Home() {
  return (
    <>
      <h1>It renders on the server.</h1>
      <p className="lede">
        View source: the markup for this page arrived in the response. React then took it over in
        the browser without rendering it a second time.
      </p>

      <ul className="cards">
        <li>
          <Link className="card" to="/posts">
            <h3>Routes with loaders →</h3>
            <p>
              Data is fetched before the component renders, on whichever side is rendering it.
            </p>
          </Link>
        </li>
        <li>
          <Link className="card" to="/posts/permissions">
            <h3>A dynamic segment →</h3>
            <p>
              <code>posts/:slug</code>, with its own loader — and a real 404 when the slug names
              nothing.
            </p>
          </Link>
        </li>
        <li>
          <Link className="card" to="/nowhere">
            <h3>A URL that matches nothing →</h3>
            <p>Status 404, rendered by the app rather than by the server's error page.</p>
          </Link>
        </li>
      </ul>

      <h2>Where to start</h2>
      <p>
        <code>src/routes.tsx</code> is the whole app in one file: paths, loaders, components and the
        page titles they produce. <code>src/server.tsx</code> is what production runs. Everything
        else exists to keep those two honest.
      </p>
    </>
  );
}
