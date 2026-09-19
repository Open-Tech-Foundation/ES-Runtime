import { BlogLayout } from "@opentf/web-docs";
import { posts } from "@opentf/web-docs/posts";
import config from "../../otfw.config.js";

export default function BlogLayoutRoute(props) {
  return (
    <BlogLayout config={config.docs} posts={posts} frame={false}>
      {props.children}
    </BlogLayout>
  );
}