// 1. named inline
export const PI = 3.14159;
export function add(a, b) { return a + b; }
export class Calculator { kind() { return "calc"; } }
// 2 + 3. named separate + alias
const E = 2.718;
const TAU = 6.283;
export { E, TAU as TAU_ALIAS };
// 5. default (separate)
function makeApp() { return { name: "app" }; }
export default makeApp;
