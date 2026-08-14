import { Link, useLoaderData } from "react-router";

import type { Post } from "../data/posts.ts";

export function Posts() {
  // Typed from the loader's return, not from a shared shape every route has to
  // fit. Each route's data is its own.
  const { posts } = useLoaderData() as { posts: Post[] };

  return (
    <>
      <h1>Writing</h1>
      <p className="lede">
        Each of these was fetched by the loader on <code>/posts</code> before this component ran.
      </p>

      <ul className="cards">
        {posts.map((post) => (
          <li key={post.slug}>
            <Link className="card" to={`/posts/${post.slug}`}>
              <h3>{post.title}</h3>
              <p>{post.summary}</p>
            </Link>
          </li>
        ))}
      </ul>
    </>
  );
}
