import BuiltWithBadge from "./BuiltWithBadge.jsx";

const OTF_ORG = "https://opentechf.org";

// Site footer: org link (logo + name) on the left, AFW badge on the right.
export default function SiteFooter() {
  return (
    <footer className="afwd-footer border-t border-zinc-800 bg-zinc-950 text-zinc-400">
      <div className="afwd-footer-inner">
        <div className="site-footer-org">
          <a
            href={OTF_ORG}
            target="_blank"
            rel="noreferrer"
            className="site-footer-org-link text-zinc-400 hover:text-zinc-200"
          >
            <img
              src="/img/otf-logo.svg"
              alt=""
              width="24"
              height="24"
              className="site-footer-org-logo"
            />
            <span>© Open Tech Foundation</span>
          </a>
          <span className="site-footer-license text-zinc-500">— Apache-2.0</span>
        </div>
        <BuiltWithBadge />
      </div>
    </footer>
  );
}