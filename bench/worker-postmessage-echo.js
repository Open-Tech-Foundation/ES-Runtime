// The worker half of bench/worker-postmessage.js: echo the payload's length so
// the round trip is measured without the reply itself dominating it.
self.onmessage = (e) => postMessage(e.data.byteLength ?? 0);
