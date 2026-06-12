// The driven event loop: timers + promises settle before esrun exits.
console.log("start");

await new Promise((resolve) => {
  setTimeout(() => {
    console.log("...fired after ~50ms");
    resolve();
  }, 50);
});

let count = 0;
const id = setInterval(() => {
  count += 1;
  console.log("tick", count);
  if (count === 3) clearInterval(id);
}, 20);

// Keep the script alive until the interval is done.
await new Promise((resolve) => setTimeout(resolve, 100));
console.log("done");
