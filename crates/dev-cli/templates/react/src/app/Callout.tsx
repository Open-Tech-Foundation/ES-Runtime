/**
 * A component that owns its styling, via CSS Modules.
 *
 * `styles` is an object the build generated: `{ box: "box_a1b2c3d4", … }`. The
 * class names in `Callout.module.css` are rewritten to match, so this
 * component's `.box` cannot collide with any other component's `.box` — which
 * is the problem CSS Modules exists to solve, since CSS itself has exactly one
 * global namespace.
 *
 * The scoped names are the same on the server and in the browser (they are
 * derived from the file's path, not from a counter), so the markup this renders
 * during SSR hydrates without a mismatch.
 *
 * Nothing is injected at runtime: the build collects every module's CSS into
 * one stylesheet and links it from the document, so it is fetched in parallel
 * with the bundle rather than after it.
 */
import type { ReactNode } from "react";

import styles from "./Callout.module.css";

export function Callout({ icon = "→", children }: { icon?: string; children: ReactNode }) {
  return (
    <aside className={styles.box}>
      <span className={styles.icon} aria-hidden="true">
        {icon}
      </span>
      <p className={styles.body}>{children}</p>
    </aside>
  );
}
