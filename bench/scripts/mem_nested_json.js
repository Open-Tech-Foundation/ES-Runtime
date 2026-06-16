try {
  let a = [];
  for (let i = 0; i < 200000; i++) {
    a = [a];
  }
  JSON.stringify(a);
} catch (e) {
  // Gracefully caught
}
