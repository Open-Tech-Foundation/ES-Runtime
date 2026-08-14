/**
 * The headers every response carries.
 *
 * A runtime that denies by default should not hand out a template whose server
 * grants everything back at the HTTP layer. These are the four that cost
 * nothing and prevent a whole class of bug each, plus a Content-Security-Policy
 * strict enough to be worth having.
 *
 * # The nonce
 *
 * One inline `<script>` is unavoidable here: react-router serialises the data
 * its loaders produced into the document so the browser can hydrate without
 * fetching it all again. A CSP that allows inline script at all
 * (`'unsafe-inline'`) allows every injected one too, which is most of what a
 * CSP is for.
 *
 * So the policy names a **nonce** instead: a value minted per response, put on
 * the one script we emitted, and required of any other. An injected script does
 * not have it and does not run — and it cannot be guessed, because it is 128
 * bits from the system CSPRNG and never reused.
 */

/** A fresh nonce for one response. */
export function nonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  // base64, because that is the alphabet the CSP grammar accepts unquoted.
  return btoa(String.fromCharCode(...bytes));
}

/**
 * Security headers for an HTML response.
 *
 * `connect-src 'self'` is what lets react-router fetch on a client-side
 * navigation. Widen it when this app starts calling an API on another origin —
 * and widen only that, rather than reaching for `default-src *`.
 */
export function securityHeaders(scriptNonce: string): Record<string, string> {
  return {
    "content-security-policy": [
      "default-src 'self'",
      `script-src 'self' 'nonce-${scriptNonce}'`,
      // No inline styles are emitted, so this needs no nonce. React sets style
      // attributes rather than <style> elements, and `style-src` does not
      // govern those — `style-src-attr` would, and is left at its default.
      "style-src 'self'",
      "img-src 'self' data:",
      "font-src 'self'",
      "connect-src 'self'",
      // Nothing here embeds anything or is meant to be embedded.
      "frame-ancestors 'none'",
      "object-src 'none'",
      // Stops a plain `<base href>` injection from re-pointing every relative
      // URL on the page at somebody else's server.
      "base-uri 'none'",
      // A form that posts somewhere else is a phishing page wearing this one's
      // address bar.
      "form-action 'self'",
    ].join("; "),
    // The browser respects the declared content type instead of sniffing the
    // bytes and deciding a .txt upload is really HTML.
    "x-content-type-options": "nosniff",
    // A path can name a resource, an id or a token. None of that belongs in the
    // Referer header of a request to another site.
    "referrer-policy": "strict-origin-when-cross-origin",
    // Features this app does not use, denied so an injected script cannot use
    // them either.
    "permissions-policy": "camera=(), microphone=(), geolocation=(), interest-cohort=()",
  };
}
