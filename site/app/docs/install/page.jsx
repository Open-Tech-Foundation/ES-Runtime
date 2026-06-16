import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import InstallBox from "../../../components/InstallBox.jsx";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

const UPGRADE = `# Built in — downloads the latest release and replaces the binary.
esrun upgrade`;

const UNINSTALL_UNIX = `# Remove the install dir, then drop the PATH line from your shell profile.
rm -rf "$HOME/.esrun"`;

const UNINSTALL_WIN = `# Remove the install dir, then remove it from your user PATH.
Remove-Item -Recurse -Force "$HOME\\.esrun"`;

export default function InstallDoc() {
  return (
    <DocsShell active="/docs/install">
      <p className="text-sm font-medium text-brand-600">Getting started</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Installation
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        A prebuilt, checksum-verified <code className="font-mono">esrun</code> binary is downloaded and installed automatically.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Install</h2>
      <div className="mt-4">
        <InstallBox />
      </div>
      <p className="mt-3 text-sm text-zinc-500">
        Prefer to build from source? See the{" "}
        <a href={GITHUB} target="_blank" rel="noreferrer" className="font-medium text-brand-600 hover:text-brand-700">
          README
        </a>
        .
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Upgrade</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">esrun upgrade</code> updates the binary in
        place — no reinstall needed.
      </p>
      <div className="mt-4">
        <CodeBlock code={UPGRADE} title="Terminal" lang="sh" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Uninstall</h2>
      <p className="mt-3 text-zinc-600">
        Delete the install directory and remove it from your <code className="font-mono">PATH</code>.
      </p>
      <div className="mt-4">
        <CodeBlock code={UNINSTALL_UNIX} title="Linux / macOS" lang="sh" />
      </div>
      <div className="mt-3">
        <CodeBlock code={UNINSTALL_WIN} title="Windows (PowerShell)" lang="sh" />
      </div>
    </DocsShell>
  );
}
