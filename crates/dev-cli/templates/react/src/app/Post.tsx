import { Link, useLoaderData } from "react-router";

import type { Post as PostData } from "../data/posts.ts";

export function Post() {
  const { post } = useLoaderData() as { post: PostData };

  return (
    <article>
      <p className="meta">
        <time dateTime={post.published}>
          {new Date(post.published).toLocaleDateString("en", {
            year: "numeric",
            month: "long",
            day: "numeric",
          })}
        </time>
      </p>
      <h1>{post.title}</h1>
      <p className="lede">{post.summary}</p>

      {post.body.map((paragraph) => (
        <p key={paragraph.slice(0, 32)}>{paragraph}</p>
      ))}

      <p>
        <Link to="/posts">← All writing</Link>
      </p>
    </article>
  );
}
