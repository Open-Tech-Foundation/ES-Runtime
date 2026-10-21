// The worker half of workers.js: answer with the payload's length.
self.onmessage = (e) => postMessage(e.data.byteLength);
