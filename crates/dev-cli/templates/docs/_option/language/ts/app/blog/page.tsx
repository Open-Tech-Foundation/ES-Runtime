import { PostList } from "@opentf/afw-docs";
import { posts } from "@opentf/afw-docs/posts";

export const metadata = { title: "Blog" };

export default function BlogIndex() {
  return (
    <div>
      <h1>Blog</h1>
      <PostList posts={posts} />
    </div>
  );
}
