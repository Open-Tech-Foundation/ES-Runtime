import { BlogLayout } from "@opentf/afw-docs";
import { posts } from "@opentf/afw-docs/posts";
import config from "../../afw.config.js";

export default function BlogLayoutRoute(props) {
  return (
    <BlogLayout config={config.docs} posts={posts} frame={false}>
      {props.children}
    </BlogLayout>
  );
}