// Imports a CommonJS package — esrun should reject it (ESM packages only).
import "oldlib";

console.log("should not reach here");
