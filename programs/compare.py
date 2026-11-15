"""Compare the programs results of the rucc kernel with those of the gcc kernel.

    python3 compare.py REFERENCE_DIR OTHER_DIR

Each directory is the OUT of run.sh. The other kernel fails the comparison when a step that
passed on the reference did not pass on it, when a set key has a value the reference does not
have (a failed unit, test or stressor), when its dmesg has a kind of bad line the reference's
does not, or when the median of a rate is more than 5% under the reference's, the one-ratio
limit of rk bench. The report goes to standard output as Markdown, and the exit status is 1
when the comparison fails.
"""

import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path

# Keys whose values are compared as sets.
SETS = {
    "failed-unit",
    "git-failed",
    "python-failed",
    "stress-failed",
    "fs-failed",
    "podman-failed",
    "failed",
}

# Keys whose values are rates, higher is better.
RATES = {"nginx-rps", "nginx-big-rps", "pgbench-tps"}

# The most a rate can be under the reference's, as a ratio.
LIMIT = 1.05

# The dmesg lines that are trouble, whatever the kernel.
BAD = re.compile(
    r"BUG:|Oops|WARNING:|general protection|soft lockup|hard LOCKUP|blocked for more than"
    r"|rcu: INFO|self-detected stall|UBSAN|KASAN|corruption|refcount_t|Kernel panic"
)


def load(directory):
    """The results of one run, as {step: {key: [values]}}."""
    results = defaultdict(lambda: defaultdict(list))
    path = Path(directory) / "results.tsv"
    if path.exists():
        for line in path.read_text().splitlines():
            parts = line.split("\t", 2)
            if len(parts) == 3:
                results[parts[0]][parts[1]].append(parts[2])
    return results


def bad_lines(directory):
    """The kinds of bad dmesg lines, with times, numbers and addresses taken out."""
    path = Path(directory) / "dmesg.txt"
    kinds = set()
    if path.exists():
        for line in path.read_text(errors="replace").splitlines():
            line = re.sub(r"^\[[ 0-9.]*\] ", "", line)
            if BAD.search(line):
                kinds.add(re.sub(r"0x[0-9a-f]+|\b[0-9]+\b", "N", line).strip())
    return kinds


def rate(values):
    numbers = []
    for value in values:
        try:
            numbers.append(float(value))
        except ValueError:
            pass
    return statistics.median(numbers) if numbers else None


def main():
    reference, other = sys.argv[1], sys.argv[2]
    ours, theirs = load(other), load(reference)
    problems = []
    rows = []
    for step in list(dict.fromkeys(list(theirs) + list(ours))):
        a, b = theirs.get(step, {}), ours.get(step, {})
        for key in list(dict.fromkeys(list(a) + list(b))):
            left, right = a.get(key, []), b.get(key, [])
            if key == "status":
                fine = right == ["0"] or left != ["0"]
                rows.append((step, key, " ".join(left), " ".join(right), fine))
                if not fine:
                    problems.append(f"{step} passed on the reference and ended {right} here")
            elif key in SETS:
                new = sorted(set(right) - set(left))
                rows.append((step, key, len(set(left)), len(set(right)), not new))
                if new:
                    problems.append(f"{step} {key} only here: {', '.join(new)}")
            elif key in RATES:
                x, y = rate(left), rate(right)
                fine = x is None or (y is not None and y * LIMIT >= x)
                ratio = f" ({y / x:.3f})" if x and y else ""
                rows.append((step, key, x, f"{y}{ratio}", fine))
                if not fine:
                    problems.append(f"{step} {key} median {y} against {x}")
            else:
                rows.append((step, key, " ".join(left), " ".join(right), True))
    dmesg = sorted(bad_lines(other) - bad_lines(reference))
    if dmesg:
        problems.append("dmesg lines only here: " + "; ".join(dmesg[:20]))
    print("| step | key | gcc | rucc | |\n|---|---|---|---|---|")
    for step, key, x, y, fine in rows:
        print(f"| {step} | {key} | {x} | {y} | {'' if fine else 'worse'} |")
    print()
    if problems:
        print("The rucc kernel does worse than the gcc kernel:\n")
        for problem in problems:
            print(f"- {problem}")
        return 1
    print("The rucc kernel does as well as the gcc kernel on every step.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
