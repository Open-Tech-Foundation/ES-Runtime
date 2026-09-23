import { PostList } from "@opentf/web-docs";
import { posts } from "@opentf/web-docs/posts";

export const metadata = { title: "Blog" };

export default function BlogIndex() {
  return (
    <div>
      <h1>Blog</h1>
      <PostList posts={posts} />
    </div>
  );
}
