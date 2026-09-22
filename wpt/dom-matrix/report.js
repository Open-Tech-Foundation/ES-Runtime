export function equivalent(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

export function outcome(result) {
  return Object.hasOwn(result, "error")
    ? { error: result.error }
    : { result: result.result };
}

export function classify(test, values) {
  if (test.limit && equivalent(values.esdev, test.expectedEsdev)) {
    return "intentional-limit";
  }
  return equivalent(values.esdev, values.chrome) ? "match" : "gap";
}

// A case drifts when its recorded columns no longer describe the run. Only the
// four outcomes are compared: `status` is derived from them, and the case's own
// metadata lives in cases.js rather than in the record.
export function drifted(recorded, values) {
  if (!recorded) return true;
  return Object.keys(values).some((runtime) => !equivalent(recorded[runtime], values[runtime]));
}
