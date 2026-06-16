try {
  let s = "a";
  for (let i = 0; i < 35; i++) {
    s += s;
  }
} catch (e) {
  // Gracefully caught
}
