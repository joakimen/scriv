#!/usr/bin/env python3
"""Compare wall-clock time of scriv builds on `list`.

Usage: bench.py <runs> <label>=<binary> [<label>=<binary> ...]
Example: bench.py 30 go=/tmp/scriv-go rust=target/release/scriv
"""
import subprocess
import sys
import time


def bench(binary: str, runs: int) -> list[float]:
    subprocess.run([binary, "list"], capture_output=True)  # warm the FS cache
    samples = []
    for _ in range(runs):
        start = time.perf_counter()
        subprocess.run([binary, "list"], capture_output=True)
        samples.append(time.perf_counter() - start)
    return samples


def main() -> None:
    runs = int(sys.argv[1])
    targets = [arg.split("=", 1) for arg in sys.argv[2:]]
    for label, binary in targets:
        s = sorted(bench(binary, runs))
        mean = 1000 * sum(s) / len(s)
        print(f"{label:<8} min {1000*s[0]:6.1f}  median {1000*s[len(s)//2]:6.1f}  "
              f"mean {mean:6.1f} ms  (n={runs})")


if __name__ == "__main__":
    main()
