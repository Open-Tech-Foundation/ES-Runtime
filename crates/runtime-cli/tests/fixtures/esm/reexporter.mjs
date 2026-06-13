export { PI, add } from "./exporter.mjs";                  // 7 re-export named
export { PI as PI2, add as add2 } from "./exporter.mjs";   // 8 re-export alias
export { default } from "./exporter.mjs";                  // 9 re-export default
export { default as Maker } from "./exporter.mjs";         // 10 re-export default as named
export * from "./more.mjs";                                // 11 export *
export * as more from "./more.mjs";                        // 12 export * as namespace
