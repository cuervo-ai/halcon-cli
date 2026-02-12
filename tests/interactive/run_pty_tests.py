#!/usr/bin/env python3
"""
Phase 18 Advanced REPL Interactive Tests.

Uses PTY-based interaction to test the cuervo REPL with a live provider.
Requires: Ollama running with deepseek-coder-v2, or OPENAI_API_KEY set.

Usage:
    python3 tests/interactive/run_pty_tests.py [--provider ollama] [--model deepseek-coder-v2]

Scenarios:
    C.0: Multi-file code generation
    C.1: Architecture design (reasoning depth)
    C.2: Codebase research (tool use)
    C.3: Debugging scenario (file edit)
    C.4: Multi-turn refinement (context retention)
    C.5: Stress test (15+ rounds, compaction)
"""

import os
import sys
import pty
import time
import select
import subprocess
import argparse

BINARY = os.path.join(os.path.dirname(__file__), '..', '..', 'target', 'release', 'cuervo')
TIMEOUT = 60  # seconds per scenario
RESULTS = {}


def read_until(fd, pattern, timeout=TIMEOUT):
    """Read from fd until pattern is found or timeout."""
    output = b""
    deadline = time.time() + timeout
    while time.time() < deadline:
        ready, _, _ = select.select([fd], [], [], 1.0)
        if ready:
            try:
                chunk = os.read(fd, 4096)
                if not chunk:
                    break
                output += chunk
                if pattern.encode() in output:
                    return output.decode(errors='replace'), True
            except OSError:
                break
    return output.decode(errors='replace'), False


def send_input(fd, text):
    """Send text to PTY."""
    os.write(fd, (text + '\n').encode())
    time.sleep(0.3)


def run_scenario(name, prompts, checks, provider='ollama', model='deepseek-coder-v2'):
    """Run a single REPL scenario via PTY."""
    print(f"\n{'='*60}")
    print(f"  Scenario: {name}")
    print(f"{'='*60}")

    env = os.environ.copy()
    env['CUERVO_LOG'] = 'error'

    args = [BINARY, 'chat', '--provider', provider, '--model', model]

    master_fd, slave_fd = pty.openpty()
    proc = subprocess.Popen(
        args,
        stdin=slave_fd,
        stdout=slave_fd,
        stderr=slave_fd,
        env=env,
    )
    os.close(slave_fd)

    try:
        # Wait for REPL prompt
        output, found = read_until(master_fd, '>', timeout=10)
        if not found:
            print(f"  SKIP: REPL didn't start (no provider?)")
            RESULTS[name] = 'SKIP'
            return

        all_output = output
        for prompt in prompts:
            print(f"  > {prompt[:80]}...")
            send_input(master_fd, prompt)
            # Wait for response (look for next prompt indicator)
            response, _ = read_until(master_fd, '>', timeout=TIMEOUT)
            all_output += response

        # Send /exit
        send_input(master_fd, '/exit')
        time.sleep(1)

        # Run checks
        passed = True
        for check_name, check_fn in checks.items():
            ok = check_fn(all_output)
            status = 'PASS' if ok else 'FAIL'
            print(f"  [{status}] {check_name}")
            if not ok:
                passed = False

        RESULTS[name] = 'PASS' if passed else 'FAIL'

    except Exception as e:
        print(f"  ERROR: {e}")
        RESULTS[name] = 'ERROR'
    finally:
        proc.terminate()
        os.close(master_fd)
        proc.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description='Phase 18 REPL Interactive Tests')
    parser.add_argument('--provider', default='ollama', help='Provider name')
    parser.add_argument('--model', default='deepseek-coder-v2', help='Model name')
    args = parser.parse_args()

    if not os.path.exists(BINARY):
        print(f"Binary not found at {BINARY}. Run `cargo build --release` first.")
        sys.exit(1)

    # C.0: Multi-file Code Generation
    run_scenario(
        'C.0: Multi-file Code Generation',
        ['Create a Rust Calculator struct with add, sub, mul, div methods. Write it to /tmp/cuervo_test_calc/lib.rs with unit tests.'],
        {
            'has_response': lambda o: len(o) > 100,
            'mentions_struct': lambda o: 'struct' in o.lower() or 'Calculator' in o,
        },
        provider=args.provider, model=args.model,
    )

    # C.1: Architecture Design
    run_scenario(
        'C.1: Architecture Design',
        ['Design a message queue system with producer, consumer, and broker. Explain components, data flow, and failure handling. Be specific about Rust types.'],
        {
            'has_response': lambda o: len(o) > 200,
            'mentions_producer': lambda o: 'producer' in o.lower(),
            'mentions_consumer': lambda o: 'consumer' in o.lower(),
        },
        provider=args.provider, model=args.model,
    )

    # C.2: Codebase Research
    run_scenario(
        'C.2: Codebase Research',
        ['Explore the cuervo codebase and explain how the resilience system works. Read the actual source code files.'],
        {
            'has_response': lambda o: len(o) > 200,
        },
        provider=args.provider, model=args.model,
    )

    # C.3: Debugging Scenario
    # First create a buggy file
    os.makedirs('/tmp/cuervo_test_debug', exist_ok=True)
    with open('/tmp/cuervo_test_debug/buggy.rs', 'w') as f:
        f.write('''fn sum_to_n(n: usize) -> usize {
    let mut total = 0;
    for i in 0..n {  // off-by-one: should be 0..=n
        total += i;
    }
    total
}
''')

    run_scenario(
        'C.3: Debugging Scenario',
        ['Read /tmp/cuervo_test_debug/buggy.rs and fix the off-by-one error in the sum_to_n function.'],
        {
            'has_response': lambda o: len(o) > 100,
            'identifies_bug': lambda o: '..=' in o or 'inclusive' in o.lower() or 'off-by-one' in o.lower(),
        },
        provider=args.provider, model=args.model,
    )

    # C.4: Multi-turn Refinement
    run_scenario(
        'C.4: Multi-turn Refinement',
        [
            'Write a Rust function that parses a CSV string into a Vec<Vec<String>>.',
            'Add error handling for malformed CSV rows.',
            'Add unit tests for the parser.',
        ],
        {
            'has_response': lambda o: len(o) > 200,
            'mentions_csv': lambda o: 'csv' in o.lower() or 'CSV' in o,
            'mentions_test': lambda o: 'test' in o.lower() or '#[test]' in o,
        },
        provider=args.provider, model=args.model,
    )

    # C.5: Stress Test (simplified — just send many prompts)
    stress_prompts = [f'What is {i} + {i}?' for i in range(1, 16)]
    run_scenario(
        'C.5: Stress Test (15 rounds)',
        stress_prompts,
        {
            'completed': lambda o: len(o) > 100,
            'no_crash': lambda o: 'panic' not in o.lower(),
        },
        provider=args.provider, model=args.model,
    )

    # Summary
    print(f"\n{'='*60}")
    print("  SUMMARY")
    print(f"{'='*60}")
    total = len(RESULTS)
    passed = sum(1 for v in RESULTS.values() if v == 'PASS')
    for name, result in RESULTS.items():
        print(f"  [{result}] {name}")
    print(f"\n  {passed}/{total} passed")

    sys.exit(0 if passed == total else 1)


if __name__ == '__main__':
    main()
