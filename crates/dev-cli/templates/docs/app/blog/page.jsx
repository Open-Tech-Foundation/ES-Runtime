// Blog index — @opentf/web-docs demo (sample post below). Linked from the top navbar.
import { Callout } from "@opentf/web-docs";
import { PostList } from "@opentf/web-docs";
import { posts } from "@opentf/web-docs/posts";

export const metadata = {
  title: "Blog (demo)",
  description: "Sample post demonstrating @opentf/web-docs blog layouts.",
};

export default function BlogIndex() {
  return (
    <div class="blog-demo">
      <h1>
        Blog <span class="blog-demo-tag">(demo)</span>
      </h1>
      <p class="blog-demo-lead">
        This section demonstrates the <code>@opentf/web-docs</code> blog feature. Remove the
        Blog nav entry, <code>app/blog/</code>, and the <code>blog</code> block in{" "}
        <code>otfw.config.js</code> if you do not need a blog.
      </p>
      <Callout type="info" title="Demo only">
        The sample post is placeholder content — replace it with your own MDX under{" "}
        <code>app/blog/&lt;slug&gt;/page.mdx</code>.
      </Callout>
      <PostList posts={posts} />
    </div>
  );
}