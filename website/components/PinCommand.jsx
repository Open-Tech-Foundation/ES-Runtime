// The pin-a-version command with today's versions filled in, for the install
// page. A component rather than a fenced block or an `export const` in the MDX:
// a fenced block interpolates nothing, and a page's exports are evaluated
// without the page's imports in scope, so a string built there from `versions`
// is undefined by the time it renders.
import { CodeBlock } from "@opentf/web-docs";

import versions from "../src/versions.js";

export default function PinCommand() {
  return (
    <CodeBlock
      lang="sh"
      name="Terminal"
      code={`ESRUN_VERSION=${versions.esrun} ESDEV_VERSION=${versions.esdev} curl -fsSL .../install.sh | bash`}
    />
  );
}
