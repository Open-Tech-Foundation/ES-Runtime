import BuiltWithBadge from "./BuiltWithBadge.jsx";

const OTF_ORG = "https://opentechf.org";
const OTF_POLICIES = `${OTF_ORG}/policies`;

// The centered policy link is positioned independently of the footer contents.
export default function SiteFooter() {
  return (
    <footer className="afwd-footer border-t border-zinc-800 bg-zinc-950 text-zinc-400">
      <div className="afwd-footer-inner site-footer-inner">
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
          <span className="site-footer-license">· Apache-2.0</span>
        </div>
        <div className="site-footer-center">
          <a
            href={OTF_POLICIES}
            target="_blank"
            rel="noopener noreferrer"
            className="site-footer-policy-link"
            aria-label="Open organization policies in a new tab"
          >
            Organization policies ↗
          </a>
        </div>
        <BuiltWithBadge />
      </div>
      <div className="site-footer-bottomline">
        No trackers, no analytics.
      </div>
    </footer>
  );
}
