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

/**
 * The `script-src` directive, which is the one that differs between a
 * development build and a real one.
 *
 * `esdev start` injects its live-reload client as an **inline** script, and it
 * cannot carry a nonce: the script is written into `dist/index.html` at build
 * time, and the nonce is minted per response. Under the production policy the
 * browser blocks it and live reload silently stops working — the page looks
 * fine and simply never updates.
 *
 * So a development build allows inline script instead. The nonce is **dropped**
 * rather than kept alongside, because a policy carrying a nonce ignores
 * `'unsafe-inline'` entirely (CSP Level 2 §4.2.2) — listing both would read
 * like belt and braces and would block the reload client exactly as before.
 */
function scriptSrc(scriptNonce: string, development: boolean): string {
  return development
    ? "script-src 'self' 'unsafe-inline'"
    : `script-src 'self' 'nonce-${scriptNonce}'`;
}

/**
 * The `connect-src` directive.
 *
 * `'self'` is what lets react-router fetch on a client-side navigation, and in
 * production it is the whole list. Widen it when this app starts calling an API
 * on another origin — and widen only this, rather than reaching for
 * `default-src *`.
 *
 * A development build also admits loopback on any port, because `esdev start`
 * serves its live-reload endpoint from one (`127.0.0.1:5173` by default) and the
 * page is served from another. Loopback rather than a wildcard: the widening
 * reaches this developer's own machine and nothing else, and it is not in the
 * production build at all.
 */
function connectSrc(development: boolean): string {
  return development
    ? "connect-src 'self' http://127.0.0.1:* http://localhost:* ws://127.0.0.1:* ws://localhost:*"
    : "connect-src 'self'";
}

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
 *
 * `development` is passed in rather than read from `process.env.NODE_ENV` here.
 * This module imports nothing, which is what makes it testable — `esdev test`
 * runs each file unbundled, so nothing has replaced that expression and there
 * is no `process` global to fall back on. A pure function takes its inputs.
 */
export function securityHeaders(
  scriptNonce: string,
  development: boolean,
): Record<string, string> {
  return {
    "content-security-policy": [
      "default-src 'self'",
      scriptSrc(scriptNonce, development),
      // No inline styles are emitted, so this needs no nonce. React sets style
      // attributes rather than <style> elements, and `style-src` does not
      // govern those — `style-src-attr` would, and is left at its default.
      "style-src 'self'",
      "img-src 'self' data:",
      "font-src 'self'",
      connectSrc(development),
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
