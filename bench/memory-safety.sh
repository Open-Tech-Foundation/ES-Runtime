#!/usr/bin/env bash
cd "$(dirname "$0")"

RUNTIMES=(node bun deno "esrun ../target/release/esrun" "llrt ../llrt")
SCRIPTS=(scripts/mem_nested_json.js scripts/mem_large_string.js scripts/mem_promise_leak.js)

for script in "${SCRIPTS[@]}"; do
  echo "Testing: $script"
  for rt in "${RUNTIMES[@]}"; do
    # extract name
    name=$(echo "$rt" | awk '{print $1}')
    
    # Run the script with a memory limit and timeout, capture exit code
    # We will just run it directly. If it segfaults, exit code will be > 128
    set +e
    timeout 10s $rt "$script" > /dev/null 2>&1
    code=$?
    set -e
    
    if [ $code -eq 0 ]; then
      res="PASS (Graceful)"
    elif [ $code -eq 124 ]; then
      res="TIMEOUT"
    elif [ $code -gt 128 ]; then
      res="CRASH (Signal $((code - 128)))"
    else
      res="ERROR (Exit $code)"
    fi
    printf "  %-10s : %s\n" "$name" "$res"
  done
done
