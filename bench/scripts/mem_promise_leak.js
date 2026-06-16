try {
  let p = Promise.resolve();
  for (let i = 0; i < 10000000; i++) {
    p = p.then(() => i);
  }
} catch (e) {
}
