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
