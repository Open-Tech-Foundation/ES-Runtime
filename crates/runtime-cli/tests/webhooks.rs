//! `Webhook` from `runtime:http` (DECISIONS D163), through the real binary: the
//! Standard Webhooks reference library's API and test vector, Ed25519 (`v1a`),
//! rotation, the timestamp window, and a delivery through `serve()`.

use std::process::{Command, Output};

mod common;

fn esrun(args: &[&str]) -> Output {
    Command::new(common::esrun_bin())
        .args(args)
        .output()
        .expect("spawn esrun")
}

fn run(args: &[&str], code: &str) -> String {
    let script = format!("-e={code}");
    let mut all: Vec<&str> = args.to_vec();
    all.push(&script);
    let out = esrun(&all);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const PRELUDE: &str = "import { Webhook, WebhookVerificationError } from 'runtime:http'; \
    const SECRET = 'whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw'; \
    const headersFor = (wh, id, body, at = new Date()) => ({ \
      'webhook-id': id, \
      'webhook-timestamp': String(Math.floor(at.getTime() / 1000)), \
      'webhook-signature': wh.sign(id, at, body), \
    }); \
    const attempt = (fn) => { try { return JSON.stringify(fn()); } catch (e) { return `${e.name}: ${e.message}`; } };";

/// The reference library's own `sign` vector, byte for byte.
#[test]
fn signing_matches_the_reference_library_vector() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const wh = new Webhook(SECRET); \
            console.log(wh.sign('msg_p5jXN8AQM9LWM0D4loKWxJek', new Date(1614265330 * 1000), '{{\"test\": 2432232314}}')); \
            console.log(new Webhook(SECRET.slice(6)).sign('msg_p5jXN8AQM9LWM0D4loKWxJek', new Date(1614265330 * 1000), '{{\"test\": 2432232314}}'));"
        ),
    );
    assert_eq!(
        s,
        "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=\n\
         v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=\n"
    );
}

#[test]
fn verify_returns_the_payload_and_refuses_everything_else() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const wh = new Webhook(SECRET); \
            const body = '{{\"event\":\"paid\",\"amount\":42}}'; \
            const h = headersFor(wh, 'msg_1', body); \
            console.log(attempt(() => wh.verify(body, h))); \
            console.log(attempt(() => wh.verify(new TextEncoder().encode(body), new Headers(h)))); \
            console.log(attempt(() => wh.verify(body, h, {{ jsonParse: false }}))); \
            console.log(attempt(() => wh.verify('', headersFor(wh, 'msg_2', '')))); \
            console.log(attempt(() => wh.verify(body.replace('42', '43'), h))); \
            console.log(attempt(() => wh.verify(body, {{ ...h, 'webhook-id': 'msg_other' }}))); \
            console.log(attempt(() => new Webhook(Webhook.generateSecret()).verify(body, h))); \
            console.log(attempt(() => wh.verify(body, {{ ...h, 'webhook-signature': 'v1,' }}))); \
            console.log(attempt(() => wh.verify(body, {{ ...h, 'webhook-signature': 'v2,abc v1,!!' }}))); \
            const {{ 'webhook-signature': _, ...unsigned }} = h; \
            console.log(attempt(() => wh.verify(body, unsigned))); \
            console.log(attempt(() => wh.verify(body, {{ ...h, 'webhook-timestamp': 'soon' }}))); \
            console.log(wh.verify(body, h) instanceof Object, (() => {{ try {{ wh.verify(body.slice(1), h); }} catch (e) {{ return e instanceof WebhookVerificationError; }} }})());"
        ),
    );
    assert_eq!(
        s,
        "{\"event\":\"paid\",\"amount\":42}\n\
         {\"event\":\"paid\",\"amount\":42}\n\
         undefined\n\
         undefined\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: Missing required headers\n\
         WebhookVerificationError: Invalid Signature Headers\n\
         true true\n"
    );
}

/// A captured delivery replayed later fails, as does one dated in the future.
#[test]
fn the_timestamp_must_be_within_the_tolerance() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const wh = new Webhook(SECRET); \
            const at = (seconds) => new Date(Date.now() + seconds * 1000); \
            console.log(attempt(() => wh.verify('{{}}', headersFor(wh, 'm', '{{}}', at(-299))))); \
            console.log(attempt(() => wh.verify('{{}}', headersFor(wh, 'm', '{{}}', at(-310))))); \
            console.log(attempt(() => wh.verify('{{}}', headersFor(wh, 'm', '{{}}', at(310))))); \
            console.log(attempt(() => wh.verify('{{}}', headersFor(wh, 'm', '{{}}', at(-30)), {{ tolerance: 10 }}))); \
            console.log(attempt(() => wh.verify('{{}}', headersFor(wh, 'm', '{{}}', at(-30)), {{ tolerance: 60 }})));"
        ),
    );
    assert_eq!(
        s,
        "{}\n\
         WebhookVerificationError: Message timestamp too old\n\
         WebhookVerificationError: Message timestamp too new\n\
         WebhookVerificationError: Message timestamp too old\n\
         {}\n"
    );
}

/// Rotation: the producer signs with both keys, a consumer holding either one
/// accepts, and a consumer holding both accepts a delivery signed by either.
#[test]
fn several_secrets_rotate_without_downtime() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const NEW = Webhook.generateSecret(); \
            const both = new Webhook([NEW, SECRET]); \
            const h = headersFor(both, 'msg_r', '{{}}'); \
            console.log(h['webhook-signature'].split(' ').length, h['webhook-signature'].split(' ').every((s) => s.startsWith('v1,'))); \
            console.log(attempt(() => new Webhook(SECRET).verify('{{}}', h)), attempt(() => new Webhook(NEW).verify('{{}}', h))); \
            const old = headersFor(new Webhook(SECRET), 'msg_o', '{{}}'); \
            console.log(attempt(() => both.verify('{{}}', old))); \
            console.log(NEW.startsWith('whsec_'), atob(NEW.slice(6)).length, Webhook.generateSecret() !== NEW);"
        ),
    );
    assert_eq!(s, "2 true\n{} {}\n{}\ntrue 32 true\n");
}

/// Ed25519: the producer keeps the secret key, consumers hold only the public
/// key, which cannot sign.
#[test]
fn asymmetric_v1a_signatures_verify_with_the_public_key_only() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const {{ secretKey, publicKey }} = Webhook.generateKeyPair(); \
            console.log(secretKey.startsWith('whsk_'), atob(secretKey.slice(5)).length, publicKey.startsWith('whpk_'), atob(publicKey.slice(5)).length); \
            const producer = new Webhook(secretKey); \
            const h = headersFor(producer, 'msg_a', '{{\"a\":1}}'); \
            console.log(h['webhook-signature'].startsWith('v1a,'), atob(h['webhook-signature'].slice(4)).length); \
            console.log(attempt(() => new Webhook(publicKey).verify('{{\"a\":1}}', h))); \
            console.log(attempt(() => producer.verify('{{\"a\":1}}', h))); \
            console.log(attempt(() => new Webhook(publicKey).verify('{{\"a\":2}}', h))); \
            console.log(attempt(() => new Webhook(Webhook.generateKeyPair().publicKey).verify('{{\"a\":1}}', h))); \
            console.log(attempt(() => new Webhook(publicKey).sign('m', new Date(), 'x'))); \
            const seedOnly = 'whsk_' + btoa(atob(secretKey.slice(5)).slice(0, 32)); \
            console.log(new Webhook(seedOnly).sign('m', new Date(0), 'x') === producer.sign('m', new Date(0), 'x')); \
            const mixed = new Webhook([secretKey, SECRET]).sign('m', new Date(0), 'x').split(' ').map((s) => s.split(',')[0]); \
            console.log(mixed.join(' '));"
        ),
    );
    assert_eq!(
        s,
        "true 64 true 32\n\
         true 64\n\
         {\"a\":1}\n\
         {\"a\":1}\n\
         WebhookVerificationError: No matching signature found\n\
         WebhookVerificationError: No matching signature found\n\
         TypeError: a whpk_ public key can only verify\n\
         true\n\
         v1a v1\n"
    );
}

/// A weak or malformed key is a mistake in the code, so it throws at once.
#[test]
fn bad_secrets_and_options_throw_type_errors() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            import {{ Secret }} from 'runtime:process'; \
            for (const make of [ \
              () => new Webhook('whsec_' + btoa('short')), \
              () => new Webhook('whsec_***'), \
              () => new Webhook([]), \
              () => new Webhook(42), \
              () => new Webhook('x'.repeat(23), {{ format: 'raw' }}), \
              () => new Webhook(SECRET, {{ format: 'hex' }}), \
              () => new Webhook(SECRET, {{ tolerance: 5 }}), \
              () => new Webhook('whpk_' + btoa('x'.repeat(31))), \
              () => new Webhook('whsk_' + btoa('x'.repeat(64))), \
              () => new Webhook(SECRET).sign('', new Date(), 'x'), \
              () => new Webhook(SECRET).sign('m', 1614265330, 'x'), \
              () => new Webhook(SECRET).verify('x', {{}}, {{ tolerance: -1 }}), \
            ]) console.log(attempt(make)); \
            console.log(attempt(() => new Webhook('x'.repeat(24), {{ format: 'raw' }}).sign('m', new Date(0), 'x').slice(0, 3))); \
            console.log(new Webhook(new Secret(SECRET)).sign('m', new Date(0), 'x') === new Webhook(SECRET).sign('m', new Date(0), 'x'));"
        ),
    );
    assert_eq!(
        s,
        "TypeError: a webhook secret must be at least 24 bytes; this one is 5\n\
         TypeError: the webhook secret is not valid base64\n\
         TypeError: Webhook needs at least one secret\n\
         TypeError: a webhook secret is a whsec_, whsk_ or whpk_ string, or bytes with { format: \"raw\" }\n\
         TypeError: a webhook secret must be at least 24 bytes; this one is 23\n\
         TypeError: format must be \"raw\" when given\n\
         TypeError: unknown Webhook option \"tolerance\"\n\
         TypeError: a whpk_ key is 32 bytes, not 31\n\
         TypeError: the whsk_ key's public half does not match its seed\n\
         TypeError: msgId must be a non-empty string\n\
         TypeError: timestamp must be a valid Date\n\
         TypeError: tolerance must be a non-negative number of seconds\n\
         \"v1,\"\n\
         true\n"
    );
}

/// A delivery through real HTTP: the producer signs the exact bytes it sends,
/// the consumer verifies `await request.text()` against `request.headers`.
#[test]
fn a_delivery_verifies_through_serve() {
    let s = run(
        &["--allow-listen", "--allow-net"],
        "import { serve, Webhook, WebhookVerificationError } from 'runtime:http'; \
        const secret = Webhook.generateSecret(); \
        const consumer = new Webhook(secret); \
        const server = serve({ hostname: '127.0.0.1', port: 0 }, async (request) => { \
          try { \
            const event = consumer.verify(await request.text(), request.headers); \
            return new Response(`ok ${event.type} ${request.headers.get('webhook-id')}`); \
          } catch (e) { \
            if (e instanceof WebhookVerificationError) return new Response(e.message, { status: 400 }); \
            throw e; \
          } \
        }); \
        const { port } = await server.addr; \
        const producer = new Webhook(secret); \
        const deliver = async (body, sign = body) => { \
          const id = 'msg_' + crypto.randomUUID(), at = new Date(); \
          const r = await fetch(`http://127.0.0.1:${port}/hooks`, { method: 'POST', body, headers: { \
            'content-type': 'application/json', 'webhook-id': id, \
            'webhook-timestamp': String(Math.floor(at / 1000)), 'webhook-signature': producer.sign(id, at, sign) } }); \
          return `${r.status} ${(await r.text()).replace(id, 'ID')}`; \
        }; \
        console.log(await deliver('{\"type\":\"invoice.paid\"}')); \
        console.log(await deliver('{\"type\":\"invoice.paid\"}', '{\"type\": \"invoice.paid\"}')); \
        await server.stop();",
    );
    assert_eq!(
        s,
        "200 ok invoice.paid ID\n400 No matching signature found\n"
    );
}
